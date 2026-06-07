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
    cert::{AuthorityId, CertHash, Certificate, Round},
    dag::DagStore,
    joint::{ValidatorId, Vote},
};

/// Cryptographically-verifiable proof that an Authority Node authored
/// two distinct certificates at the same round.
///
/// Carries the full signed certificates so that any verifier —
/// including one without access to the local `DagStore` — can
/// independently confirm equivocation by:
///
/// 1. Verifying both ML-DSA-65 signatures against the author's pubkey.
/// 2. Confirming `cert_a.author == cert_b.author` and
///    `cert_a.round == cert_b.round`.
/// 3. Confirming `cert_a.hash(network_id) != cert_b.hash(network_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EquivocationProof {
    /// Equivocating Authority Node.
    pub author: AuthorityId,
    /// Round at which the equivocation occurred.
    pub round: Round,
    /// One of the two distinct certificate hashes.
    pub cert_a: CertHash,
    /// The other distinct certificate hash. Always `cert_a != cert_b`.
    pub cert_b: CertHash,
    /// Full signed certificate backing `cert_a`. Enables independent
    /// verification without the local DAG.
    pub cert_a_signed: Certificate,
    /// Full signed certificate backing `cert_b`.
    pub cert_b_signed: Certificate,
}

/// Cryptographically-verifiable proof that a Validator Ring member
/// voted for two distinct candidates.
///
/// Carries the full signed votes so that any verifier can independently
/// confirm the double-vote by:
///
/// 1. Verifying both ML-DSA-65 signatures against the voter's pubkey.
/// 2. Confirming `vote_a.validator == vote_b.validator`.
/// 3. Confirming `vote_a.candidate != vote_b.candidate`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ValidatorEquivocationProof {
    /// Double-voting validator.
    pub validator: ValidatorId,
    /// One of the two distinct candidates.
    pub candidate_a: CertHash,
    /// The other distinct candidate. Always `candidate_a != candidate_b`.
    pub candidate_b: CertHash,
    /// Full signed vote backing `candidate_a`. Enables independent
    /// verification without the local vote set.
    pub vote_a: Vote,
    /// Full signed vote backing `candidate_b`.
    pub vote_b: Vote,
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
            let cert_a_signed = dag.get(&hashes[0]).expect("hash from bucket").clone();
            let cert_b_signed = dag.get(&hashes[1]).expect("hash from bucket").clone();
            proofs.push(EquivocationProof {
                author,
                round,
                cert_a: hashes[0],
                cert_b: hashes[1],
                cert_a_signed,
                cert_b_signed,
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
    // validator -> vec of (candidate, first vote with that candidate)
    let mut buckets: BTreeMap<ValidatorId, Vec<(CertHash, Vote)>> = BTreeMap::new();
    for v in votes {
        let entry = buckets.entry(v.validator).or_default();
        if !entry.iter().any(|(c, _)| *c == v.candidate) {
            entry.push((v.candidate, v.clone()));
        }
    }
    let mut proofs = Vec::new();
    for (validator, mut candidates) in buckets {
        if candidates.len() >= 2 {
            candidates.sort_by_key(|(c, _)| *c);
            proofs.push(ValidatorEquivocationProof {
                validator,
                candidate_a: candidates[0].0,
                candidate_b: candidates[1].0,
                vote_a: candidates[0].1.clone(),
                vote_b: candidates[1].1.clone(),
            });
        }
    }
    proofs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert::Certificate;

    const NET: &str = "test";

    #[test]
    fn honest_dag_yields_no_authority_proofs() {
        let mut dag = DagStore::new();
        for a in 0..5u32 {
            dag.insert(Certificate::genesis(a, [a as u8; 32]), NET)
                .unwrap();
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
        let h1 = c1.hash(NET);
        let h2 = c2.hash(NET);
        dag.insert(c1, NET).unwrap();
        dag.insert(c2, NET).unwrap();

        let proofs = detect_authority_equivocation(&dag);
        assert_eq!(proofs.len(), 1);
        let p = &proofs[0];
        assert_eq!(p.author, 0);
        assert_eq!(p.round, 0);
        // Sorted; the canonical pair is (min, max) of the two hashes.
        let (lo, hi) = if h1 < h2 { (h1, h2) } else { (h2, h1) };
        assert_eq!(p.cert_a, lo);
        assert_eq!(p.cert_b, hi);
    }

    #[test]
    fn honest_votes_yield_no_validator_proofs() {
        let cand = CertHash::from([1; 32]);
        let votes = vec![
            Vote {
                validator: 0,
                candidate: cand,
                signature: vec![],
            },
            Vote {
                validator: 1,
                candidate: cand,
                signature: vec![],
            },
            Vote {
                validator: 0,
                candidate: cand,
                signature: vec![],
            }, // duplicate, not double
        ];
        assert!(detect_validator_double_vote(&votes).is_empty());
    }

    #[test]
    fn double_voter_produces_proof() {
        let a = CertHash::from([1; 32]);
        let b = CertHash::from([2; 32]);
        let votes = vec![
            Vote {
                validator: 0,
                candidate: a,
                signature: vec![],
            },
            Vote {
                validator: 0,
                candidate: b,
                signature: vec![],
            }, // double
            Vote {
                validator: 1,
                candidate: a,
                signature: vec![],
            },
        ];
        let proofs = detect_validator_double_vote(&votes);
        assert_eq!(proofs.len(), 1);
        let p = &proofs[0];
        assert_eq!(p.validator, 0);
        assert_eq!(p.vote_a.candidate, a);
        assert_eq!(p.vote_b.candidate, b);
    }
}
