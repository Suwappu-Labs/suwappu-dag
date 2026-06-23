//! LTP corridor super-node attestation pipeline (paper §10).
//!
//! "Source-chain state is verified, not asserted. Super-node BFT
//! quorums attest to observed source-chain finality under ML-KEM-768-
//! sealed session keys and aggregate signatures." Per §10, the
//! corridor quorum is sized to require **seven-of-nine** witnesses to
//! produce a forged attestation.
//!
//! Sprint scope (DAG-S15): the BLS12-381 aggregate-signature surface
//! and the 7-of-9 attestation pipeline. The ML-KEM-768 sealed-session-
//! key half of the constant-size on-chain commitment (paper §10.2)
//! lands with the cross-chain DID STARK pipeline in DAG-S17.
//!
//! ## Pipeline
//!
//! 1. The sender publishes an `AttestationPayload` (source chain,
//!    target chain, source height, state root, timestamp).
//! 2. Each corridor witness signs `H_c(payload)` with their BLS12-381
//!    secret key, producing a `WitnessSignature`.
//! 3. `attest(corridor, payload, signatures)` aggregates the
//!    signatures, verifies the count meets `LTP_ATTESTATION_QUORUM_THRESHOLD`
//!    (7), and verifies every signature individually before aggregating.
//! 4. `verify_attestation(corridor, attestation)` checks that the
//!    aggregate signature verifies under the aggregate public key of
//!    the named signers and that `|signers| ≥ 7`.

use std::collections::BTreeSet;

use blst::min_pk::{PublicKey as BlsPublicKey, Signature as BlsSignature};
use serde::{Deserialize, Serialize};
use suwappu_crypto::{bls, hash};
use thiserror::Error;

use crate::{LTP_ATTESTATION_QUORUM_SIZE, LTP_ATTESTATION_QUORUM_THRESHOLD};

/// Identifier of a regulated corridor (paper §9). The corridor is the
/// unit on which 7-of-9 BFT super-node attestation is sized.
pub type CorridorId = u32;

/// Identifier of a chain in the cross-chain settlement surface.
pub type ChainId = u64;

/// Authority identifier — index into the Authority Ring. Re-declared
/// here to avoid a cyclic dep on `suwappu-authority`; the production wire-
/// up binds these to `suwappu_authority::AuthorityId`.
pub type AuthorityId = u32;

/// A corridor super-node (subset of the Authority Ring that additionally
/// serves as an LTP attestation witness for a specific jurisdictional
/// corridor — paper §9 Definition 4 role 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuperNode {
    /// Authority Ring identifier of the operator.
    pub authority: AuthorityId,
    /// Corridor this super-node serves.
    pub corridor: CorridorId,
    /// BLS12-381 public key (G1, min-pubkey-size variant).
    pub bls_public_key: Vec<u8>,
}

/// A corridor — exactly `LTP_ATTESTATION_QUORUM_SIZE` (9) super-nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Corridor {
    /// Corridor identifier.
    pub id: CorridorId,
    /// Witnesses. Must contain exactly `LTP_ATTESTATION_QUORUM_SIZE`
    /// entries with distinct `authority` ids.
    pub members: Vec<SuperNode>,
}

impl Corridor {
    /// Resolve a member's BLS public key by authority id.
    pub fn member_pubkey(&self, authority: AuthorityId) -> Option<BlsPublicKey> {
        let bytes = self
            .members
            .iter()
            .find(|m| m.authority == authority)
            .map(|m| m.bls_public_key.as_slice())?;
        BlsPublicKey::from_bytes(bytes).ok()
    }
}

/// What the witnesses are signing — observed source-chain finality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationPayload {
    /// Source chain id whose state is being attested.
    pub source_chain: ChainId,
    /// Target chain id that will receive the attestation.
    pub target_chain: ChainId,
    /// Block height at which the source-chain state is observed.
    pub source_height: u64,
    /// State root on the source chain at `source_height`.
    pub state_root: [u8; 32],
    /// Round/timestamp at which the witness observed finality.
    pub timestamp_round: u64,
}

impl AttestationPayload {
    /// Canonical SHA3-256 digest used as the BLS signing message
    /// (paper §10.2: `H_c` is the canonical-path hash, SHA3-256).
    pub fn canonical_digest(&self) -> [u8; 32] {
        let mut blob = Vec::with_capacity(8 + 8 + 8 + 32 + 8);
        blob.extend_from_slice(&self.source_chain.to_be_bytes());
        blob.extend_from_slice(&self.target_chain.to_be_bytes());
        blob.extend_from_slice(&self.source_height.to_be_bytes());
        blob.extend_from_slice(&self.state_root);
        blob.extend_from_slice(&self.timestamp_round.to_be_bytes());
        hash::sha3_256_domain(b"SUWAPPU-LTP-ATTEST-V1", &blob)
    }
}

/// A single witness's BLS12-381 signature over `payload.canonical_digest()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WitnessSignature {
    /// Signing super-node (by Authority Ring id).
    pub witness: AuthorityId,
    /// BLS12-381 signature bytes (96 B for min-pubkey-size variant).
    pub signature: Vec<u8>,
}

/// Aggregated corridor attestation. Constant-size on-wire: aggregate
/// BLS signature (≈96 B) + signers bitmap (2 B for 9 witnesses) +
/// payload digest (32 B) ≈ 130 B before the ML-KEM-768 envelope of
/// §10.2 wraps it for the constant 1,600 B on-chain commitment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorridorAttestation {
    /// The payload signed by every member of `signers`.
    pub payload: AttestationPayload,
    /// BLS12-381 aggregate signature bytes.
    pub aggregate_signature: Vec<u8>,
    /// Authority ids of the witnesses that signed. Length must be
    /// at least `LTP_ATTESTATION_QUORUM_THRESHOLD` and at most
    /// `LTP_ATTESTATION_QUORUM_SIZE`.
    pub signers: BTreeSet<AuthorityId>,
}

/// Errors emitted by the attestation pipeline.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum LtpError {
    /// Corridor membership is not exactly the 9-witness slot.
    #[error("corridor must have exactly {expected} members, got {got}")]
    BadCorridorSize {
        /// Required size (9).
        expected: usize,
        /// Observed size.
        got: usize,
    },

    /// Fewer than `LTP_ATTESTATION_QUORUM_THRESHOLD` witnesses signed.
    #[error("ltp quorum below threshold: have {have}, need {need}")]
    BelowQuorum {
        /// Distinct signers supplied.
        have: usize,
        /// Quorum threshold required (7).
        need: usize,
    },

    /// A signature carried a witness id absent from the corridor.
    #[error("signature from unknown witness {0}")]
    UnknownWitness(AuthorityId),

    /// A signature failed verification under the witness's BLS public key.
    #[error("invalid signature from witness {0}")]
    InvalidSignature(AuthorityId),

    /// Aggregate-signature verification failed (the per-witness checks
    /// passed but the aggregate did not — should only fire under
    /// signature-byte corruption).
    #[error("aggregate signature verification failed")]
    AggregateVerificationFailed,

    /// A `WitnessSignature::signature` byte payload was not a valid
    /// BLS12-381 signature encoding.
    #[error("malformed signature bytes from witness {0}")]
    MalformedSignature(AuthorityId),

    /// A corridor member's `bls_public_key` did not parse.
    #[error("malformed public key for witness {0}")]
    MalformedKey(AuthorityId),
}

/// Aggregate `signatures` into a `CorridorAttestation` over `payload`.
///
/// Validation order:
/// 1. Corridor size is `LTP_ATTESTATION_QUORUM_SIZE` (9).
/// 2. Every signing witness is in `corridor`.
/// 3. Distinct-signer count meets `LTP_ATTESTATION_QUORUM_THRESHOLD` (7).
/// 4. Each individual signature verifies over `payload.canonical_digest()`.
/// 5. Aggregate signature is computed and re-verified.
pub fn attest(
    corridor: &Corridor,
    payload: AttestationPayload,
    signatures: Vec<WitnessSignature>,
) -> Result<CorridorAttestation, LtpError> {
    if corridor.members.len() != LTP_ATTESTATION_QUORUM_SIZE {
        return Err(LtpError::BadCorridorSize {
            expected: LTP_ATTESTATION_QUORUM_SIZE,
            got: corridor.members.len(),
        });
    }

    let digest = payload.canonical_digest();

    // Build the signer set, dedup, and verify each individual signature.
    let mut signers: BTreeSet<AuthorityId> = BTreeSet::new();
    let mut parsed_sigs: Vec<BlsSignature> = Vec::new();
    let mut parsed_pks: Vec<BlsPublicKey> = Vec::new();
    for sig in &signatures {
        if !signers.insert(sig.witness) {
            continue; // duplicate witness — ignore extras
        }
        let pk = corridor
            .member_pubkey(sig.witness)
            .ok_or(LtpError::UnknownWitness(sig.witness))?;
        let bls_sig = BlsSignature::from_bytes(&sig.signature)
            .map_err(|_| LtpError::MalformedSignature(sig.witness))?;
        bls::verify(&digest, &bls_sig, &pk).map_err(|_| LtpError::InvalidSignature(sig.witness))?;
        parsed_sigs.push(bls_sig);
        parsed_pks.push(pk);
    }

    if signers.len() < LTP_ATTESTATION_QUORUM_THRESHOLD {
        return Err(LtpError::BelowQuorum {
            have: signers.len(),
            need: LTP_ATTESTATION_QUORUM_THRESHOLD,
        });
    }

    let sig_refs: Vec<&BlsSignature> = parsed_sigs.iter().collect();
    let pk_refs: Vec<&BlsPublicKey> = parsed_pks.iter().collect();
    let agg_sig = bls::aggregate(&sig_refs).map_err(|_| LtpError::AggregateVerificationFailed)?;
    let agg_pk =
        bls::aggregate_pubkeys(&pk_refs).map_err(|_| LtpError::AggregateVerificationFailed)?;
    bls::verify_aggregate(&digest, &agg_sig, &agg_pk)
        .map_err(|_| LtpError::AggregateVerificationFailed)?;

    Ok(CorridorAttestation {
        payload,
        aggregate_signature: agg_sig.to_bytes().to_vec(),
        signers,
    })
}

/// Verify a `CorridorAttestation` against the corridor membership.
///
/// 1. Signer count meets the 7-of-9 threshold.
/// 2. Every signer is a corridor member.
/// 3. The aggregate signature verifies over the canonical payload
///    digest under the aggregate of the named signers' public keys.
pub fn verify_attestation(
    corridor: &Corridor,
    attestation: &CorridorAttestation,
) -> Result<(), LtpError> {
    if corridor.members.len() != LTP_ATTESTATION_QUORUM_SIZE {
        return Err(LtpError::BadCorridorSize {
            expected: LTP_ATTESTATION_QUORUM_SIZE,
            got: corridor.members.len(),
        });
    }
    if attestation.signers.len() < LTP_ATTESTATION_QUORUM_THRESHOLD {
        return Err(LtpError::BelowQuorum {
            have: attestation.signers.len(),
            need: LTP_ATTESTATION_QUORUM_THRESHOLD,
        });
    }

    let mut pks: Vec<BlsPublicKey> = Vec::new();
    for signer in &attestation.signers {
        let pk = corridor
            .member_pubkey(*signer)
            .ok_or(LtpError::UnknownWitness(*signer))?;
        pks.push(pk);
    }
    let pk_refs: Vec<&BlsPublicKey> = pks.iter().collect();
    let agg_pk =
        bls::aggregate_pubkeys(&pk_refs).map_err(|_| LtpError::AggregateVerificationFailed)?;

    let digest = attestation.payload.canonical_digest();
    let agg_sig = BlsSignature::from_bytes(&attestation.aggregate_signature)
        .map_err(|_| LtpError::AggregateVerificationFailed)?;
    bls::verify_aggregate(&digest, &agg_sig, &agg_pk)
        .map_err(|_| LtpError::AggregateVerificationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_corridor() -> (Corridor, Vec<blst::min_pk::SecretKey>) {
        let mut members = Vec::with_capacity(9);
        let mut sks = Vec::with_capacity(9);
        for i in 0..9u32 {
            let (pk, sk) = bls::keypair();
            members.push(SuperNode {
                authority: i,
                corridor: 0,
                bls_public_key: pk.to_bytes().to_vec(),
            });
            sks.push(sk);
        }
        (Corridor { id: 0, members }, sks)
    }

    fn sample_payload() -> AttestationPayload {
        AttestationPayload {
            source_chain: 1,
            target_chain: 100,
            source_height: 42,
            state_root: [0xAB; 32],
            timestamp_round: 7,
        }
    }

    #[test]
    fn seven_signatures_attest_and_verify() {
        let (corridor, sks) = build_corridor();
        let payload = sample_payload();
        let digest = payload.canonical_digest();

        let mut sigs = Vec::with_capacity(7);
        for i in 0..7u32 {
            let s = bls::sign(&digest, &sks[i as usize]);
            sigs.push(WitnessSignature {
                witness: i,
                signature: s.to_bytes().to_vec(),
            });
        }

        let att = attest(&corridor, payload, sigs).unwrap();
        verify_attestation(&corridor, &att).unwrap();
        assert_eq!(att.signers.len(), 7);
    }

    #[test]
    fn six_signatures_below_quorum() {
        let (corridor, sks) = build_corridor();
        let payload = sample_payload();
        let digest = payload.canonical_digest();
        let mut sigs = Vec::with_capacity(6);
        for i in 0..6u32 {
            let s = bls::sign(&digest, &sks[i as usize]);
            sigs.push(WitnessSignature {
                witness: i,
                signature: s.to_bytes().to_vec(),
            });
        }
        let err = attest(&corridor, payload, sigs);
        assert!(matches!(
            err,
            Err(LtpError::BelowQuorum { have: 6, need: 7 })
        ));
    }

    #[test]
    fn unknown_witness_rejected() {
        let (corridor, _sks) = build_corridor();
        let (_pk_foreign, sk_foreign) = bls::keypair();
        let payload = sample_payload();
        let digest = payload.canonical_digest();
        let s = bls::sign(&digest, &sk_foreign);
        let sigs = vec![WitnessSignature {
            witness: 99,
            signature: s.to_bytes().to_vec(),
        }];
        let err = attest(&corridor, payload, sigs);
        assert_eq!(err, Err(LtpError::UnknownWitness(99)));
    }

    #[test]
    fn bad_corridor_size_rejected() {
        let mut corridor = build_corridor().0;
        corridor.members.truncate(8); // 8 ≠ 9
        let payload = sample_payload();
        let err = attest(&corridor, payload, Vec::new());
        assert!(matches!(err, Err(LtpError::BadCorridorSize { .. })));
    }

    #[test]
    fn tampered_payload_breaks_verification() {
        let (corridor, sks) = build_corridor();
        let payload = sample_payload();
        let digest = payload.canonical_digest();
        let mut sigs = Vec::with_capacity(7);
        for i in 0..7u32 {
            let s = bls::sign(&digest, &sks[i as usize]);
            sigs.push(WitnessSignature {
                witness: i,
                signature: s.to_bytes().to_vec(),
            });
        }
        let mut att = attest(&corridor, payload, sigs).unwrap();
        // Tamper the payload — recompute the canonical digest.
        att.payload.state_root[0] ^= 1;
        let err = verify_attestation(&corridor, &att);
        assert_eq!(err, Err(LtpError::AggregateVerificationFailed));
    }
}
