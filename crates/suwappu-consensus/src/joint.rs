//! Joint-quorum AND-gate (paper Theorem 2).
//!
//! A commit is *jointly ratified* iff both rings reach Byzantine-fault-
//! tolerant supermajority on the same candidate:
//!
//! 1. **Authority leg.** The S4 commit rule (`commit_leader`) returns
//!    `Some(hash)` — at least `quorum_threshold(n)` distinct Authority
//!    Ring members support the candidate at round `r+1`.
//! 2. **Validator leg.** The aggregate stake of Validators who vote for
//!    that hash exceeds two-thirds of the total Validator-Ring stake.
//!
//! Theorem 2 (paper §11): a safety violation — two distinct certificates
//! `c_1 ≠ c_2` both jointly ratified at the same round — requires
//! Byzantine corruption of *both* rings simultaneously. Specifically:
//!
//! * The intersection of Authority supporters of `c_1` and `c_2` (the
//!   *equivocator* set) is at least `⌈|A|/3⌉` — Authority equivocation
//!   reaches the BFT threshold.
//! * The intersection of Validator voters for `c_1` and `c_2`, weighted
//!   by stake (the *double-voter stake*), strictly exceeds `total/3` —
//!   Validator equivocation reaches the BFT threshold.
//!
//! Verified at 10,000 cases by `joint_quorum_safety`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    cert::{AuthorityId, CertHash, Round},
    commit::{commit_leader, CommitteeSize},
    dag::DagStore,
};

/// Validator-Ring participant identifier — index into the published
/// Validator Ring set. Distinct from `AuthorityId`; a single physical
/// operator may be in both rings but the identifiers are kept separate
/// at the consensus layer.
pub type ValidatorId = u32;

/// Stake weight, in canonical SUWAPPU units. Phase-1 phase uses `u128` to
/// admit up to the 50,000-SUWAPPU × 500-validator Validator Ring envelope
/// without saturation; the canonical unit lands with the validator-set
/// registry in DAG-S6.
pub type Stake = u128;

/// Validator Ring stake table.
///
/// Phase-1 keeps this as an in-memory map; the on-chain registry lands
/// in DAG-S6 alongside cert-signature verification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StakeTable {
    weights: BTreeMap<ValidatorId, Stake>,
}

impl StakeTable {
    /// Construct an empty stake table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct from an iterator of `(ValidatorId, Stake)` pairs. Later
    /// entries overwrite earlier ones for the same id.
    pub fn from_entries<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (ValidatorId, Stake)>,
    {
        let mut t = Self::new();
        for (id, w) in entries {
            t.weights.insert(id, w);
        }
        t
    }

    /// Insert or overwrite a single validator's stake.
    pub fn insert(&mut self, id: ValidatorId, stake: Stake) {
        self.weights.insert(id, stake);
    }

    /// Remove a validator from the stake table. Returns the prior stake
    /// if the id was seated, otherwise `None`. Used by the daemon at
    /// commit time to reflect `ExitAuthority` / `EjectAuthority` intents
    /// and slashing ejections (DAG-S27.3 / S27.4).
    pub fn remove(&mut self, id: &ValidatorId) -> Option<Stake> {
        self.weights.remove(id)
    }

    /// Total stake across the Validator Ring.
    pub fn total(&self) -> Stake {
        self.weights.values().copied().sum()
    }

    /// Borrow a validator's stake; zero if absent.
    pub fn weight(&self, id: ValidatorId) -> Stake {
        self.weights.get(&id).copied().unwrap_or(0)
    }

    /// Iterate validator ids in canonical (ascending) order.
    pub fn ids(&self) -> impl Iterator<Item = ValidatorId> + '_ {
        self.weights.keys().copied()
    }
}

/// A Validator Ring vote for a candidate commit.
///
/// Phase-1 votes carry no signature; cert signature verification lands
/// in DAG-S6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Vote {
    /// The voter.
    pub validator: ValidatorId,
    /// The candidate certificate hash being ratified.
    pub candidate: CertHash,
}

/// Stake-weighted Validator-Ring quorum threshold: strictly greater than
/// two-thirds of total stake. Matches paper Definition 2.
pub fn validator_quorum_threshold(stake_table: &StakeTable) -> Stake {
    // Integer "strictly greater than 2/3" → `>= floor(2*total/3) + 1`.
    let two_thirds = (2 * stake_table.total()) / 3;
    two_thirds + 1
}

/// Aggregate stake voting for `candidate`. Counts each `ValidatorId` at
/// most once even if duplicate votes are present.
pub fn voting_stake(stake_table: &StakeTable, candidate: CertHash, votes: &[Vote]) -> Stake {
    let mut counted: BTreeSet<ValidatorId> = BTreeSet::new();
    let mut total: Stake = 0;
    for v in votes {
        if v.candidate == candidate && counted.insert(v.validator) {
            total += stake_table.weight(v.validator);
        }
    }
    total
}

/// `true` iff `candidate` has Validator-quorum stake support.
pub fn validator_quorum_met(stake_table: &StakeTable, candidate: CertHash, votes: &[Vote]) -> bool {
    voting_stake(stake_table, candidate, votes) >= validator_quorum_threshold(stake_table)
}

/// Run the joint-quorum AND-gate at `round`.
///
/// Returns `Some(hash)` iff the Authority-side `commit_leader` ratifies a
/// candidate at `round` *and* the Validator Ring stake-weighted quorum
/// votes for that same candidate.
pub fn joint_commit(
    dag: &DagStore,
    round: Round,
    committee: CommitteeSize,
    stake_table: &StakeTable,
    votes: &[Vote],
) -> Option<CertHash> {
    let candidate = commit_leader(dag, round, committee)?;
    if validator_quorum_met(stake_table, candidate, votes) {
        Some(candidate)
    } else {
        None
    }
}

/// Authority equivocator set for two conflicting candidates.
///
/// An Authority Node *equivocates* between `cand_a` and `cand_b` at
/// round `r` iff it authored a supporting certificate at round `r+1`
/// that lists `cand_a` as a parent *and* a (different) supporting
/// certificate at round `r+1` that lists `cand_b` as a parent. Returns
/// the set of equivocating Authority identifiers.
pub fn authority_equivocators(
    dag: &DagStore,
    cand_a: CertHash,
    cand_b: CertHash,
    round: Round,
) -> BTreeSet<AuthorityId> {
    let support_a: BTreeSet<AuthorityId> = dag
        .linearize()
        .into_iter()
        .filter_map(|h| dag.get(&h).map(|c| (h, c)))
        .filter(|(_, c)| c.round == round && c.parents.contains(&cand_a))
        .map(|(_, c)| c.author)
        .collect();
    let support_b: BTreeSet<AuthorityId> = dag
        .linearize()
        .into_iter()
        .filter_map(|h| dag.get(&h).map(|c| (h, c)))
        .filter(|(_, c)| c.round == round && c.parents.contains(&cand_b))
        .map(|(_, c)| c.author)
        .collect();
    support_a.intersection(&support_b).copied().collect()
}

/// Validator double-vote stake for two conflicting candidates.
///
/// A Validator *double-votes* if it casts a `Vote` for both `cand_a` and
/// `cand_b`. Returns the aggregate stake of double-voters.
pub fn validator_double_vote_stake(
    stake_table: &StakeTable,
    cand_a: CertHash,
    cand_b: CertHash,
    votes: &[Vote],
) -> Stake {
    let voters_a: BTreeSet<ValidatorId> = votes
        .iter()
        .filter(|v| v.candidate == cand_a)
        .map(|v| v.validator)
        .collect();
    let voters_b: BTreeSet<ValidatorId> = votes
        .iter()
        .filter(|v| v.candidate == cand_b)
        .map(|v| v.validator)
        .collect();
    voters_a
        .intersection(&voters_b)
        .map(|id| stake_table.weight(*id))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert::Certificate;

    fn genesis(author: u32) -> Certificate {
        Certificate::genesis(author, [author as u8; 32])
    }

    fn child(author: u32, round: Round, parents: Vec<CertHash>, tag: u8) -> Certificate {
        Certificate {
            author,
            round,
            parents,
            payload_digest: [tag; 32],
        }
    }

    fn equal_stake_table(n_validators: u32, per_stake: Stake) -> StakeTable {
        StakeTable::from_entries((0..n_validators).map(|i| (i as ValidatorId, per_stake)))
    }

    #[test]
    fn empty_table_quorum_threshold_is_one() {
        let t = StakeTable::new();
        assert_eq!(t.total(), 0);
        // (2 * 0) / 3 + 1 = 1
        assert_eq!(validator_quorum_threshold(&t), 1);
    }

    #[test]
    fn quorum_threshold_strictly_above_two_thirds() {
        let t = equal_stake_table(9, 100); // total 900
                                           // 2/3 of 900 is 600; strictly > 2/3 means ≥ 601.
        assert_eq!(validator_quorum_threshold(&t), 601);
    }

    #[test]
    fn voting_stake_dedups_double_votes() {
        let t = equal_stake_table(3, 100); // total 300
        let cand = CertHash([0x11; 32]);
        let votes = vec![
            Vote {
                validator: 0,
                candidate: cand,
            },
            Vote {
                validator: 0,
                candidate: cand,
            }, // duplicate
            Vote {
                validator: 1,
                candidate: cand,
            },
        ];
        assert_eq!(voting_stake(&t, cand, &votes), 200);
    }

    #[test]
    fn joint_commit_requires_both_legs() {
        // n_authorities = 1: trivially commits Authority-side with a
        // single supporter. Then bolt on a Validator quorum.
        let mut dag = DagStore::new();
        let g = dag.insert(genesis(0)).unwrap();
        dag.insert(child(0, 1, vec![g], 0xAA)).unwrap();

        let stake = equal_stake_table(3, 100); // total 300, threshold 201
        let cand_hash = g;
        // 3 votes × 100 = 300 ≥ 201
        let votes = vec![
            Vote {
                validator: 0,
                candidate: cand_hash,
            },
            Vote {
                validator: 1,
                candidate: cand_hash,
            },
            Vote {
                validator: 2,
                candidate: cand_hash,
            },
        ];
        assert_eq!(joint_commit(&dag, 0, 1, &stake, &votes), Some(cand_hash));

        // Drop one vote → stake 200 < 201 → no joint commit.
        let votes_short = votes[..2].to_vec();
        assert_eq!(joint_commit(&dag, 0, 1, &stake, &votes_short), None);
    }

    #[test]
    fn authority_equivocators_detects_overlap() {
        let mut dag = DagStore::new();
        let g0 = dag.insert(genesis(0)).unwrap();
        let g1 = dag.insert(genesis(1)).unwrap();
        // Author 0 supports both g0 and g1 at round 1 by issuing two
        // different round-1 certs. That equivocates.
        dag.insert(child(0, 1, vec![g0], 0xA0)).unwrap();
        // (In a real DAG this would also be a per-round equivocation,
        // caught in DAG-S7. Here we only test the joint-quorum lens.)
        let cert_dup = Certificate {
            author: 0,
            round: 1,
            parents: vec![g1],
            payload_digest: [0xB0; 32],
        };
        dag.insert(cert_dup).unwrap();

        let equivocators = authority_equivocators(&dag, g0, g1, 1);
        assert_eq!(equivocators, BTreeSet::from([0]));
    }

    #[test]
    fn validator_double_vote_stake_sums_overlap() {
        let stake = equal_stake_table(4, 50); // total 200
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
            }, // double-voter
            Vote {
                validator: 1,
                candidate: a,
            },
            Vote {
                validator: 2,
                candidate: b,
            },
            Vote {
                validator: 3,
                candidate: a,
            },
            Vote {
                validator: 3,
                candidate: b,
            }, // double-voter
        ];
        assert_eq!(validator_double_vote_stake(&stake, a, b, &votes), 100);
    }
}
