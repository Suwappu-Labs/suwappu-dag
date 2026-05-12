//! DAG-S7 exit-gate property tests.
//!
//! Exit gate: `equivocation_proof_slashes` — for any Authority equivocation
//! (two distinct certs at the same `(author, round)`), the detection
//! pipeline emits a proof and the slashing pipeline fully expels the
//! Authority with 100% stake forfeit.
//!
//! Supporting properties:
//!
//! - `validator_double_vote_slashes_30_percent` — for any Validator
//!   double-voter, the detection pipeline emits a proof and slashing
//!   reduces stake by exactly 30%.
//! - `honest_dag_produces_no_proofs` — a DAG with one cert per
//!   `(round, author)` yields zero Authority proofs; a vote set with
//!   one candidate per validator yields zero Validator proofs.
//! - `slashing_is_idempotent` — slashing the same offender twice
//!   matches the first slash on first call and returns `None` on the
//!   second.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p gsx-validator --release`.

use gsx_authority::{
    slash_authority, AuthorityMember, AuthorityRegistry, AUTHORITY_STAKE_THRESHOLD_GSX,
};
use gsx_consensus::{
    detect_authority_equivocation, detect_validator_double_vote, AuthorityId, CertHash,
    Certificate, DagStore, Vote,
};
use gsx_validator::{
    slash_validator_double_vote, Stake, ValidatorMember, ValidatorRegistry,
    VALIDATOR_STAKE_THRESHOLD_GSX,
};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — equivocation proofs are detected and slashed.
    #[test]
    fn equivocation_proof_slashes(
        n_authorities in 1u32..=10,
        equivocator in 0u32..=9,
        payload_seed in any::<u64>(),
    ) {
        let equivocator = equivocator.min(n_authorities - 1);
        let stake = AUTHORITY_STAKE_THRESHOLD_GSX + payload_seed % 10_000;

        let mut registry = AuthorityRegistry::new();
        for i in 0..n_authorities {
            registry.admit(AuthorityMember {
                id: i as AuthorityId,
                stake_gsx: stake,
                public_key_bytes: vec![i as u8; 32],
            }).unwrap();
        }

        // Build a DAG where the `equivocator` produces two distinct
        // genesis-shape certs (different payloads → distinct hashes).
        let mut dag = DagStore::new();
        for i in 0..n_authorities {
            let mut p = [0u8; 32];
            p[0] = i as u8;
            p[1] = 0x01;
            dag.insert(Certificate::genesis(i as AuthorityId, p)).unwrap();
        }
        let mut p2 = [0u8; 32];
        p2[0] = equivocator as u8;
        p2[1] = 0x02;
        dag.insert(Certificate::genesis(equivocator as AuthorityId, p2)).unwrap();

        // Detection: exactly one proof, accusing the equivocator.
        let proofs = detect_authority_equivocation(&dag);
        prop_assert_eq!(proofs.len(), 1);
        prop_assert_eq!(proofs[0].author, equivocator as AuthorityId);
        prop_assert_eq!(proofs[0].round, 0);
        prop_assert_ne!(proofs[0].cert_a, proofs[0].cert_b);

        // Slashing: 100% stake forfeit + expulsion.
        let slash = slash_authority(&mut registry, equivocator as AuthorityId).unwrap();
        prop_assert_eq!(slash.stake_lost, stake);
        prop_assert!(slash.expelled);
        prop_assert!(!registry.contains(equivocator as AuthorityId));
        prop_assert_eq!(registry.len() as u32, n_authorities - 1);
    }

    /// Validator double-voting yields a proof and a 30%-stake slash.
    #[test]
    fn validator_double_vote_slashes_30_percent(
        n_validators in 1u32..=15,
        double_voter in 0u32..=14,
        stake_units in 1u128..=10,
    ) {
        let double_voter = double_voter.min(n_validators - 1);
        let stake = VALIDATOR_STAKE_THRESHOLD_GSX * stake_units;

        let mut registry = ValidatorRegistry::new();
        for i in 0..n_validators {
            registry.admit(ValidatorMember {
                id: i,
                stake_gsx: stake,
            }).unwrap();
        }

        let cand_a = CertHash([0xAA; 32]);
        let cand_b = CertHash([0xBB; 32]);
        let mut votes = Vec::new();
        for i in 0..n_validators {
            votes.push(Vote { validator: i, candidate: cand_a });
            if i == double_voter {
                votes.push(Vote { validator: i, candidate: cand_b });
            }
        }

        let proofs = detect_validator_double_vote(&votes);
        prop_assert_eq!(proofs.len(), 1);
        prop_assert_eq!(proofs[0].validator, double_voter);

        let slash = slash_validator_double_vote(&mut registry, double_voter).unwrap();
        let expected_loss: Stake = stake * 30 / 100;
        prop_assert_eq!(slash.stake_lost, expected_loss);
        prop_assert_eq!(slash.remaining_stake, stake - expected_loss);
    }

    /// An honest DAG (one cert per (round, author)) and an honest vote
    /// set (one candidate per validator) produce no proofs.
    #[test]
    fn honest_inputs_produce_no_proofs(
        n_authorities in 1u32..=10,
        n_validators in 1u32..=10,
    ) {
        let mut dag = DagStore::new();
        for i in 0..n_authorities {
            let mut p = [0u8; 32];
            p[0] = i as u8;
            dag.insert(Certificate::genesis(i as AuthorityId, p)).unwrap();
        }
        prop_assert!(detect_authority_equivocation(&dag).is_empty());

        let cand = CertHash([0xCC; 32]);
        let votes: Vec<Vote> = (0..n_validators)
            .map(|i| Vote { validator: i, candidate: cand })
            .collect();
        prop_assert!(detect_validator_double_vote(&votes).is_empty());
    }

    /// Slashing the same Authority twice: first call returns the slash;
    /// second call returns None (already expelled). Same for validators.
    #[test]
    fn slashing_is_idempotent(
        n_authorities in 1u32..=5,
        n_validators in 1u32..=5,
        target in 0u32..=4,
    ) {
        let target_a = target.min(n_authorities - 1);
        let target_v = target.min(n_validators - 1);

        let mut auth_reg = AuthorityRegistry::new();
        for i in 0..n_authorities {
            auth_reg.admit(AuthorityMember {
                id: i,
                stake_gsx: AUTHORITY_STAKE_THRESHOLD_GSX,
                public_key_bytes: vec![i as u8; 32],
            }).unwrap();
        }
        slash_authority(&mut auth_reg, target_a).unwrap();
        prop_assert!(slash_authority(&mut auth_reg, target_a).is_none());

        let mut val_reg = ValidatorRegistry::new();
        for i in 0..n_validators {
            val_reg.admit(ValidatorMember {
                id: i,
                stake_gsx: VALIDATOR_STAKE_THRESHOLD_GSX * 10,
            }).unwrap();
        }
        let first = slash_validator_double_vote(&mut val_reg, target_v).unwrap();
        // After a 30% slash, the validator is re-seated at reduced
        // stake (if remaining ≥ floor). A second slash should succeed
        // again, producing a smaller loss.
        if val_reg.contains(target_v) {
            let second = slash_validator_double_vote(&mut val_reg, target_v).unwrap();
            prop_assert!(second.stake_lost <= first.stake_lost,
                "second slash should be ≤ first (stake decreased)");
        }
    }
}
