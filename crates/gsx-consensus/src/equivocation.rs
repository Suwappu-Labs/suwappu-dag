//! Equivocation detection over the certificate DAG and the Validator
//! vote set.
//!
//! Paper §6.4 (fast-path) and §5.2 (Validator slashing) require
//! cryptographically-verifiable proofs of misbehaviour that downstream
//! slashing pipelines can consume.
//!
//! **Authority equivocation.** An Authority Node equivocates at round
//! `r` iff it authored two distinct certificates at `r`. DAG-S3's
//! `DagStore` accepts both because their content hashes differ (the
//! payload digest or parent set differs); the proof is the pair of
//! distinct cert hashes for the same `(author, round)`.
//!
//! **Validator double-voting.** A Validator Ring member double-votes iff
//! it casts `Vote`s for two distinct candidates at the same height. The
//! proof is the pair of distinct candidate hashes for the same
//! validator id.
//!
//! Verified at 10,000 cases by `equivocation_proof_slashes` (exit gate)
//! and supporting properties.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    cert::{AuthorityId, CertHash, Round},
    dag::DagStore,
    joint::{ValidatorId, Vote},
};

/// Cryptographically-verifiable proof that an Authority Node authored
/// two distinct certificates at the same round.
///
/// Phase-1 phase carries the two `CertHash` values; production wraps
/// the original `Certificate` payloads plus their ML-DSA-65 signatures
/// so a verifier without the local `DagStore` can independently
/// reconstruct the proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EquivocationProof {
    /// Equivocating Authority Node.
    pub author: AuthorityId,
    /// Round at which the equivocation occurred.
    pub round: Round,
    /// One of the two distinct certificate hashes.
    pub cert_a: CertHash,
    /// The other distinct certificate hash. Always `cert_a != cert_b`.
    pub cert_b: CertHash,
}

/// Cryptographically-verifiable proof that a Validator Ring member
/// voted for two distinct candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ValidatorEquivocationProof {
    /// Double-voting validator.
    pub validator: ValidatorId,
    /// One of the two distinct candidates.
    pub candidate_a: CertHash,
    /// The other distinct candidate. Always `candidate_a != candidate_b`.
    pub candidate_b: CertHash,
}

/// Walk the DAG and return one proof per equivocating Authority. If an
/// author produced more than two distinct certificates at the same
/// round, returns a proof containing the two lexicographically-smallest
/// hashes; additional equivocations are folded into the same author's
/// slashing event downstream.
pub fn detect_authority_equivocation(dag: &DagStore) -> Vec<EquivocationProof> {
    // (author, round) -> sorted vec of cert hashes seen
    let mut buckets: BTreeMap<(AuthorityId, Round), Vec<CertHash>> = BTreeMap::new();
    for h in dag.linearize() {
        if let Some(c) = dag.get(&h) {
            buckets.entry((c.author, c.round)).or_default().push(h);
        }
    }
    let mut proofs = Vec::new();
    for ((author, round), mut hashes) in buckets {
        if hashes.len() >= 2 {
            hashes.sort();
            proofs.push(EquivocationProof {
                author,
                round,
                cert_a: hashes[0],
                cert_b: hashes[1],
            });
        }
    }
    proofs
}

/// Scan a vote set and return one proof per double-voting validator.
///
/// Conservative semantics: a validator who casts multiple votes for the
/// *same* candidate is not flagged (duplicate votes are deduped by
/// `voting_stake`). Only distinct candidates trigger a proof.
pub fn detect_validator_double_vote(votes: &[Vote]) -> Vec<ValidatorEquivocationProof> {
    // validator -> sorted vec of distinct candidate hashes
    let mut buckets: BTreeMap<ValidatorId, Vec<CertHash>> = BTreeMap::new();
    for v in votes {
        let entry = buckets.entry(v.validator).or_default();
        if !entry.contains(&v.candidate) {
            entry.push(v.candidate);
        }
    }
    let mut proofs = Vec::new();
    for (validator, mut candidates) in buckets {
        if candidates.len() >= 2 {
            candidates.sort();
            proofs.push(ValidatorEquivocationProof {
                validator,
                candidate_a: candidates[0],
                candidate_b: candidates[1],
            });
        }
    }
    proofs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert::Certificate;

    #[test]
    fn honest_dag_yields_no_authority_proofs() {
        let mut dag = DagStore::new();
        for a in 0..5u32 {
            dag.insert(Certificate::genesis(a, [a as u8; 32])).unwrap();
        }
        assert!(detect_authority_equivocation(&dag).is_empty());
    }

    #[test]
    fn equivocating_author_produces_proof() {
        let mut dag = DagStore::new();
        // Author 0 produces two genesis-shape certs by varying the
        // payload digest — distinct hashes, same (author, round).
        let c1 = Certificate::genesis(0, [0xA1; 32]);
        let c2 = Certificate::genesis(0, [0xB2; 32]);
        let h1 = c1.hash();
        let h2 = c2.hash();
        dag.insert(c1).unwrap();
        dag.insert(c2).unwrap();

        let proofs = detect_authority_equivocation(&dag);
        assert_eq!(proofs.len(), 1);
        let p = proofs[0];
        assert_eq!(p.author, 0);
        assert_eq!(p.round, 0);
        // Sorted; the canonical pair is (min, max) of the two hashes.
        let (lo, hi) = if h1 < h2 { (h1, h2) } else { (h2, h1) };
        assert_eq!(p.cert_a, lo);
        assert_eq!(p.cert_b, hi);
    }

    #[test]
    fn honest_votes_yield_no_validator_proofs() {
        let cand = CertHash([1; 32]);
        let votes = vec![
            Vote {
                validator: 0,
                candidate: cand,
            },
            Vote {
                validator: 1,
                candidate: cand,
            },
            Vote {
                validator: 0,
                candidate: cand,
            }, // duplicate, not double
        ];
        assert!(detect_validator_double_vote(&votes).is_empty());
    }

    #[test]
    fn double_voter_produces_proof() {
        let a = CertHash([1; 32]);
        let b = CertHash([2; 32]);
        let votes = vec![
            Vote {
                validator: 0,
                candidate: a,
            },
            Vote {
                validator: 0,
                candidate: b,
            }, // double
            Vote {
                validator: 1,
                candidate: a,
            },
        ];
        let proofs = detect_validator_double_vote(&votes);
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].validator, 0);
    }
}
