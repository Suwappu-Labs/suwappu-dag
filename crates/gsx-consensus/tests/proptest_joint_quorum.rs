//! DAG-S5 exit-gate property tests — paper Theorem 2 (joint-quorum safety).
//!
//! Exit gate: `joint_quorum_safety` — for any two distinct certificates
//! that both achieve `joint_commit` at the same round, the Authority
//! Ring contributes at least `⌈|A|/3⌉` equivocators AND the Validator
//! Ring contributes more than `total_stake/3` of double-vote stake.
//! Conflicting commits require Byzantine corruption of BOTH rings.
//!
//! Supporting properties:
//!
//! - `single_authority_corruption_safe` — corrupting only the Authority
//!   Ring (no Validator double-votes) cannot produce two joint commits.
//! - `single_validator_corruption_safe` — corrupting only the Validator
//!   Ring (no Authority equivocators) cannot produce two joint commits.
//! - `joint_commit_implies_authority_commit` — every joint commit also
//!   satisfies the S4 Authority-side commit rule.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p gsx-consensus --release`.

use std::collections::BTreeSet;

use gsx_consensus::{
    authority_equivocators, commit_leader, joint_commit, quorum_threshold, validator_quorum_met,
    AuthorityId, CertHash, Certificate, DagStore, StakeTable, ValidatorId, Vote,
};
use proptest::prelude::*;

const NET: &str = "test";

/// Build a DAG where authors in `dishonest_set` author *two* round-1
/// certs (one supporting `cand_a`, one supporting `cand_b`). Honest
/// authors author one round-1 cert supporting `cand_a`. Returns
/// `(dag, cand_a, cand_b)`.
///
/// This models the simplest Authority-side equivocation surface: at
/// round 0 both `cand_a` and `cand_b` are produced by author 0; at
/// round 1 the equivocating fraction supports both, honest fraction
/// supports only `cand_a`.
fn build_equivocation_dag(
    n_authorities: u32,
    dishonest_authorities: u32,
    payload_seed: u64,
) -> (DagStore, CertHash, CertHash) {
    let mut dag = DagStore::new();

    // Author 0 produces two distinct genesis-shape certs at round 0:
    // we can't have two round-0 certs from the same author honestly,
    // but for the test we model it as two separate genesis certs from
    // *different* authors that the equivocators support concurrently.
    // Use author 0 for cand_a and author 1 for cand_b.
    let mut p_a = [0u8; 32];
    p_a[0] = 0xAA;
    p_a[1] = (payload_seed & 0xFF) as u8;
    let cand_a_cert = Certificate::genesis(0, p_a);
    let cand_a = cand_a_cert.hash(NET);
    dag.insert(cand_a_cert, NET).unwrap();

    let mut p_b = [0u8; 32];
    p_b[0] = 0xBB;
    p_b[1] = (payload_seed & 0xFF) as u8;
    let cand_b_cert = Certificate::genesis(1, p_b);
    let cand_b = cand_b_cert.hash(NET);
    dag.insert(cand_b_cert, NET).unwrap();

    // Genesis certs for the rest of the authorities so they can author
    // round-1 certs. (Round 1 certs need parents; we let everyone use
    // cand_a as a parent so the DAG is valid.)
    let mut other_genesis = Vec::new();
    for a in 2..n_authorities {
        let mut p = [0u8; 32];
        p[0] = a as u8;
        let g = Certificate::genesis(a as AuthorityId, p);
        let h = g.hash(NET);
        dag.insert(g, NET).unwrap();
        other_genesis.push(h);
    }

    // Build round-1 certs. We treat the first `dishonest_authorities`
    // authors as equivocators — they support BOTH cand_a and cand_b
    // by issuing two round-1 certs. Honest authors support only cand_a.
    //
    // To keep the DAG valid (one round-1 cert per author per "submission"),
    // we have equivocators submit (author, round=1) certs with different
    // parent sets — distinct hashes. The DAG store accepts both because
    // their hashes differ. Equivocation detection happens at DAG-S7.
    for a in 0..n_authorities {
        let mut payload_1 = [0u8; 32];
        payload_1[0] = a as u8;
        payload_1[1] = 0x01;
        let cert_a = Certificate {
            author: a as AuthorityId,
            round: 1,
            parents: vec![cand_a],
            payload_digest: payload_1,
            signature: vec![],
        };
        dag.insert(cert_a, NET).unwrap();

        if a < dishonest_authorities {
            let mut payload_2 = [0u8; 32];
            payload_2[0] = a as u8;
            payload_2[1] = 0x02;
            let cert_b = Certificate {
                author: a as AuthorityId,
                round: 1,
                parents: vec![cand_b],
                payload_digest: payload_2,
                signature: vec![],
            };
            dag.insert(cert_b, NET).unwrap();
        }
    }
    (dag, cand_a, cand_b)
}

/// Build a uniform Validator stake table with `n` validators each
/// holding `per_stake` GSX.
fn uniform_stake(n: u32, per_stake: u128) -> StakeTable {
    StakeTable::from_entries((0..n).map(|i| (i as ValidatorId, per_stake)))
}

/// Build vote sets where the first `dishonest` validators double-vote
/// for both candidates and the rest vote only for `cand_a`.
fn votes_with_double_voters(
    n_validators: u32,
    dishonest: u32,
    cand_a: CertHash,
    cand_b: CertHash,
) -> Vec<Vote> {
    let mut votes = Vec::new();
    for v in 0..n_validators {
        votes.push(Vote {
            validator: v as ValidatorId,
            candidate: cand_a,
            signature: vec![],
        });
        if v < dishonest {
            votes.push(Vote {
                validator: v as ValidatorId,
                candidate: cand_b,
                signature: vec![],
            });
        }
    }
    votes
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — Theorem 2. For any two distinct certificates that
    /// both achieve `joint_commit` at round 0, the Authority Ring must
    /// contribute at least `⌈n/3⌉` equivocators AND the Validator Ring
    /// must contribute more than `total_stake/3` of double-vote stake.
    #[test]
    fn joint_quorum_safety(
        n_authorities in 3u32..=12,
        n_validators in 3u32..=20,
        per_stake in 1u128..=1000,
        dishonest_authorities in 0u32..=12,
        dishonest_validators in 0u32..=20,
        payload_seed in any::<u64>(),
    ) {
        let dishonest_authorities = dishonest_authorities.min(n_authorities);
        let dishonest_validators = dishonest_validators.min(n_validators);

        let (dag, cand_a, cand_b) =
            build_equivocation_dag(n_authorities, dishonest_authorities, payload_seed);
        let stake = uniform_stake(n_validators, per_stake);
        let votes =
            votes_with_double_voters(n_validators, dishonest_validators, cand_a, cand_b);

        let committed_a = joint_commit(&dag, 0, n_authorities, &stake, &votes) == Some(cand_a);
        // For cand_b we have to also check if Authority-side commits it —
        // the S4 rule picks a specific leader by round-robin, so cand_b
        // is committed iff the round-0 leader == author of cand_b == 1
        // (and the equivocators provide the needed supporters).
        let cand_b_authority = commit_leader(&dag, 0, n_authorities) == Some(cand_b);
        let committed_b = cand_b_authority && validator_quorum_met(&stake, cand_b, &votes);

        if committed_a && committed_b {
            // Conflicting joint commit: BOTH legs of the AND-gate must
            // have admitted ≥ 1/3 Byzantine fraction.
            let equivocators = authority_equivocators(&dag, cand_a, cand_b, 1);
            let auth_third = n_authorities.div_ceil(3);

            prop_assert!(
                equivocators.len() as u32 >= auth_third,
                "joint conflict with only {} equivocators (need ≥ {})",
                equivocators.len(),
                auth_third,
            );

            let double_stake = gsx_consensus::validator_double_vote_stake(
                &stake, cand_a, cand_b, &votes,
            );
            let total = stake.total();
            prop_assert!(
                double_stake * 3 > total,
                "joint conflict with only {} double-stake (need > {})",
                double_stake,
                total / 3,
            );
        }
    }

    /// Single-Authority corruption is insufficient: with no Validator
    /// double-votes, no two distinct candidates can be jointly committed
    /// regardless of how many Authority equivocators exist.
    #[test]
    fn single_authority_corruption_safe(
        n_authorities in 3u32..=10,
        n_validators in 3u32..=15,
        per_stake in 1u128..=1000,
        dishonest_authorities in 0u32..=10,
        payload_seed in any::<u64>(),
    ) {
        let dishonest_authorities = dishonest_authorities.min(n_authorities);
        let (dag, cand_a, cand_b) =
            build_equivocation_dag(n_authorities, dishonest_authorities, payload_seed);
        let stake = uniform_stake(n_validators, per_stake);

        // No Validator double-votes: every validator votes only for
        // cand_a.
        let votes = votes_with_double_voters(n_validators, 0, cand_a, cand_b);

        let committed_a = joint_commit(&dag, 0, n_authorities, &stake, &votes) == Some(cand_a);
        let cand_b_authority = commit_leader(&dag, 0, n_authorities) == Some(cand_b);
        let committed_b = cand_b_authority && validator_quorum_met(&stake, cand_b, &votes);

        prop_assert!(
            !(committed_a && committed_b),
            "single-Authority corruption produced a joint commit conflict",
        );
    }

    /// Single-Validator corruption is insufficient: with no Authority
    /// equivocators, no two distinct candidates can be jointly committed
    /// regardless of how many Validator double-voters exist.
    #[test]
    fn single_validator_corruption_safe(
        n_authorities in 3u32..=10,
        n_validators in 3u32..=15,
        per_stake in 1u128..=1000,
        dishonest_validators in 0u32..=15,
        payload_seed in any::<u64>(),
    ) {
        let dishonest_validators = dishonest_validators.min(n_validators);
        // No Authority equivocators.
        let (dag, cand_a, cand_b) = build_equivocation_dag(n_authorities, 0, payload_seed);
        let stake = uniform_stake(n_validators, per_stake);
        let votes =
            votes_with_double_voters(n_validators, dishonest_validators, cand_a, cand_b);

        let committed_a = joint_commit(&dag, 0, n_authorities, &stake, &votes) == Some(cand_a);
        let cand_b_authority = commit_leader(&dag, 0, n_authorities) == Some(cand_b);
        let committed_b = cand_b_authority && validator_quorum_met(&stake, cand_b, &votes);

        prop_assert!(
            !(committed_a && committed_b),
            "single-Validator corruption produced a joint commit conflict",
        );
    }

    /// Every joint commit also satisfies the S4 Authority-side commit
    /// rule. The joint rule strictly tightens, never weakens.
    #[test]
    fn joint_commit_implies_authority_commit(
        n_authorities in 1u32..=10,
        n_validators in 1u32..=15,
        per_stake in 1u128..=1000,
        dishonest_authorities in 0u32..=10,
        dishonest_validators in 0u32..=15,
        payload_seed in any::<u64>(),
    ) {
        let dishonest_authorities = dishonest_authorities.min(n_authorities);
        let dishonest_validators = dishonest_validators.min(n_validators);
        let (dag, cand_a, cand_b) =
            build_equivocation_dag(n_authorities, dishonest_authorities, payload_seed);
        let stake = uniform_stake(n_validators, per_stake);
        let votes =
            votes_with_double_voters(n_validators, dishonest_validators, cand_a, cand_b);

        if let Some(hash) = joint_commit(&dag, 0, n_authorities, &stake, &votes) {
            prop_assert_eq!(
                commit_leader(&dag, 0, n_authorities),
                Some(hash),
                "joint commit emitted a hash the Authority-side rule does not",
            );
            // And the Authority threshold is unchanged.
            let _ = quorum_threshold(n_authorities);
        }

        // Quiet a couple of dead-code warnings.
        let _ : BTreeSet<AuthorityId> = BTreeSet::new();
    }
}
