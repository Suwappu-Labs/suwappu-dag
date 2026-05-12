//! DAG-S9 exit-gate property tests.
//!
//! Exit gate: `fast_path_equivocation_full_slash` — for any fast-path
//! certificate whose main-lane confirmation window contains a
//! conflicting transaction, the detection pipeline emits a proof and
//! `slash_fast_path_signers` slashes EVERY seated signer at 100% bonded
//! stake plus expulsion. Paper §6.4: "equivocation is slashable at 100%
//! of the offending Authority Node's bonded stake plus expulsion."
//!
//! Supporting properties:
//!
//! - `honest_consistency_produces_no_proof` — when the main lane is
//!   non-conflicting, no equivocation proof is produced.
//! - `non_signers_are_unaffected` — slashing only touches members in
//!   `cert.signers`.
//! - `repeat_slashing_is_idempotent` — running the slashing pipeline a
//!   second time on the same proof is a no-op (signers already absent).
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p gsx-fastpath --release`.

use std::collections::BTreeSet;

use gsx_authority::{AuthorityMember, AuthorityRegistry, AUTHORITY_STAKE_THRESHOLD_GSX};
use gsx_consensus::{AuthorityId, CertHash};
use gsx_fastpath::{
    certify, detect_fast_path_equivocation, fast_path_quorum_size, slash_fast_path_signers,
    FastPathTx, MainLaneTx, OwnedObjectId, OwnerAddress, FAST_PATH_CONFIRMATION_K,
};
use proptest::prelude::*;

fn build_tx(object_seed: u8, lineage_round: u64, payload_seed: u8) -> FastPathTx {
    FastPathTx {
        object: OwnedObjectId([object_seed; 32]),
        owner: OwnerAddress([0xAA; 32]),
        nonce: 0,
        lineage: CertHash([0xBB; 32]),
        lineage_round,
        payload_digest: [payload_seed; 32],
    }
}

fn seat_all(registry: &mut AuthorityRegistry, n: u32) {
    for i in 0..n {
        registry
            .admit(AuthorityMember {
                id: i,
                stake_gsx: AUTHORITY_STAKE_THRESHOLD_GSX,
                public_key_bytes: vec![i as u8; 32],
            })
            .unwrap();
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — every signer of an equivocating fast-path cert is
    /// slashed at 100% bonded stake + expulsion.
    #[test]
    fn fast_path_equivocation_full_slash(
        n_authorities in 3u32..=50,
        object_seed in any::<u8>(),
        payload_seed in any::<u8>(),
        lineage_round in 0u64..=100,
        conflict_offset in 1u32..=FAST_PATH_CONFIRMATION_K,
    ) {
        let conflict_payload = payload_seed.wrapping_add(1);
        prop_assume!(conflict_payload != payload_seed);

        // Certify with exactly the quorum signers.
        let q = fast_path_quorum_size(n_authorities);
        let signers: BTreeSet<AuthorityId> = (0..q).collect();
        let tx = build_tx(object_seed, lineage_round, payload_seed);
        let cert = certify(tx.clone(), signers.clone(), n_authorities).unwrap();

        // Plant a conflicting main-lane tx inside the binding window.
        let main_lane = vec![MainLaneTx {
            round: lineage_round + conflict_offset as u64,
            object: tx.object,
            payload_digest: [conflict_payload; 32],
            lineage: CertHash([0xCC; 32]),
        }];

        // Detection emits a proof.
        let proof = detect_fast_path_equivocation(&cert, &main_lane)
            .expect("conflicting main-lane tx must yield a proof");
        prop_assert_eq!(proof.cert.signers.len(), q as usize);
        prop_assert_eq!(proof.conflicting_tx.object, tx.object);
        prop_assert_ne!(proof.conflicting_tx.payload_digest, tx.payload_digest);

        // Seat all authorities and run the slashing pipeline.
        let mut registry = AuthorityRegistry::new();
        seat_all(&mut registry, n_authorities);
        let outcome = slash_fast_path_signers(&mut registry, &proof);

        prop_assert_eq!(outcome.slashed.len() as u32, q);
        prop_assert!(outcome.missing.is_empty());
        // Every slashed entry forfeits 100% of the stake.
        for (_id, slash) in &outcome.slashed {
            prop_assert_eq!(slash.stake_lost, AUTHORITY_STAKE_THRESHOLD_GSX);
            prop_assert!(slash.expelled);
        }
        // Total stake lost = q × floor.
        prop_assert_eq!(
            outcome.total_stake_lost(),
            (q as u64) * AUTHORITY_STAKE_THRESHOLD_GSX,
        );
        // No signer remains in the registry.
        for s in signers {
            prop_assert!(!registry.contains(s));
        }
    }

    /// An honest, non-conflicting main lane produces no equivocation
    /// proof — there is nothing to slash.
    #[test]
    fn honest_consistency_produces_no_proof(
        n_authorities in 3u32..=50,
        object_seed in any::<u8>(),
        payload_seed in any::<u8>(),
        lineage_round in 0u64..=100,
        unrelated_payload in any::<u8>(),
    ) {
        let q = fast_path_quorum_size(n_authorities);
        let signers: BTreeSet<AuthorityId> = (0..q).collect();
        let tx = build_tx(object_seed, lineage_round, payload_seed);
        let cert = certify(tx.clone(), signers, n_authorities).unwrap();

        // Main lane references DIFFERENT objects only.
        let main_lane = vec![MainLaneTx {
            round: lineage_round + 1,
            object: OwnedObjectId([object_seed.wrapping_add(1); 32]),
            payload_digest: [unrelated_payload; 32],
            lineage: CertHash([0xCC; 32]),
        }];

        prop_assert!(detect_fast_path_equivocation(&cert, &main_lane).is_none());
    }

    /// Slashing only affects members listed in `cert.signers`. Members
    /// outside that set remain seated.
    #[test]
    fn non_signers_are_unaffected(
        n_authorities in 5u32..=50,
        payload_seed in any::<u8>(),
        lineage_round in 0u64..=100,
    ) {
        let conflict_payload = payload_seed.wrapping_add(1);
        prop_assume!(conflict_payload != payload_seed);

        let q = fast_path_quorum_size(n_authorities);
        // Signers = first q authors. Non-signers = remaining n - q.
        let signers: BTreeSet<AuthorityId> = (0..q).collect();
        let tx = build_tx(0, lineage_round, payload_seed);
        let cert = certify(tx.clone(), signers.clone(), n_authorities).unwrap();

        let main_lane = vec![MainLaneTx {
            round: lineage_round + 1,
            object: tx.object,
            payload_digest: [conflict_payload; 32],
            lineage: CertHash([0xCC; 32]),
        }];
        let proof = detect_fast_path_equivocation(&cert, &main_lane).unwrap();

        let mut registry = AuthorityRegistry::new();
        seat_all(&mut registry, n_authorities);
        slash_fast_path_signers(&mut registry, &proof);

        for non_signer in q..n_authorities {
            prop_assert!(
                registry.contains(non_signer),
                "non-signer {} unexpectedly slashed",
                non_signer,
            );
        }
    }

    /// Running the slashing pipeline a second time on the same proof
    /// yields no further slashes — every signer is already gone.
    #[test]
    fn repeat_slashing_is_idempotent(
        n_authorities in 3u32..=10,
        payload_seed in any::<u8>(),
        lineage_round in 0u64..=100,
    ) {
        let conflict_payload = payload_seed.wrapping_add(1);
        prop_assume!(conflict_payload != payload_seed);

        let q = fast_path_quorum_size(n_authorities);
        let signers: BTreeSet<AuthorityId> = (0..q).collect();
        let tx = build_tx(0, lineage_round, payload_seed);
        let cert = certify(tx.clone(), signers.clone(), n_authorities).unwrap();

        let main_lane = vec![MainLaneTx {
            round: lineage_round + 1,
            object: tx.object,
            payload_digest: [conflict_payload; 32],
            lineage: CertHash([0xCC; 32]),
        }];
        let proof = detect_fast_path_equivocation(&cert, &main_lane).unwrap();

        let mut registry = AuthorityRegistry::new();
        seat_all(&mut registry, n_authorities);

        let first = slash_fast_path_signers(&mut registry, &proof);
        let second = slash_fast_path_signers(&mut registry, &proof);

        prop_assert_eq!(first.slashed.len() as u32, q);
        prop_assert!(second.slashed.is_empty());
        prop_assert_eq!(second.missing.len() as u32, q);
    }
}
