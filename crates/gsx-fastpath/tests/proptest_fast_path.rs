//! DAG-S8 exit-gate property tests.
//!
//! Exit gate: `fast_path_main_lane_consistency` — an honestly certified
//! fast-path transaction whose main-lane confirmation window contains
//! no conflicting transaction for the same object is binding.
//!
//! Supporting properties:
//!
//! - `fast_path_quorum_matches_paper` — the certification threshold is
//!   `⌈(2/3)n⌉ + 1`, matching paper §6.4.
//! - `conflict_within_k_rounds_breaks_binding` — a main-lane tx for the
//!   same object with a different payload digest within `K=4` rounds of
//!   the lineage round signals inconsistency.
//! - `conflict_beyond_k_rounds_preserves_binding` — the same conflict
//!   strictly beyond the binding window does NOT signal inconsistency
//!   (it is the main-lane Mysticeti-C commit rule's responsibility).
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p gsx-fastpath --release`.

use std::collections::BTreeSet;

use gsx_consensus::{AuthorityId, CertHash, Round};
use gsx_fastpath::{
    certify, fast_path_quorum_size, is_main_lane_consistent, FastPathTx, MainLaneTx, OwnedObjectId,
    OwnerAddress, FAST_PATH_CONFIRMATION_K,
};
use proptest::prelude::*;

fn build_tx(object_seed: u8, lineage_round: Round, payload_seed: u8) -> FastPathTx {
    FastPathTx {
        object: OwnedObjectId([object_seed; 32]),
        owner: OwnerAddress([0xAA; 32]),
        nonce: 0,
        lineage: CertHash([0xBB; 32]),
        lineage_round,
        payload_digest: [payload_seed; 32],
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — honest certification + non-conflicting main lane
    /// yields a binding fast-path certificate.
    #[test]
    fn fast_path_main_lane_consistency(
        n_authorities in 3u32..=50,
        object_seed in any::<u8>(),
        payload_seed in any::<u8>(),
        lineage_round in 0u64..=100,
        main_lane_size in 0usize..=20,
        unrelated_payload in any::<u8>(),
    ) {
        // Certify with exactly the quorum size; signers 0..q-1.
        let q = fast_path_quorum_size(n_authorities);
        let signers: BTreeSet<AuthorityId> = (0..q).collect();
        let tx = build_tx(object_seed, lineage_round, payload_seed);
        let cert = certify(tx.clone(), signers, n_authorities).expect("at-quorum certify");

        // Build a main lane that touches a DIFFERENT object inside the
        // window plus the SAME object's payload OUTSIDE the window.
        // Neither should signal a conflict.
        let mut main_lane = Vec::new();
        for i in 0..main_lane_size {
            let round = lineage_round + 1 + (i as u64 % FAST_PATH_CONFIRMATION_K as u64);
            let other_object = OwnedObjectId([object_seed.wrapping_add(1); 32]);
            main_lane.push(MainLaneTx {
                round,
                object: other_object,
                payload_digest: [unrelated_payload; 32],
                lineage: CertHash([0xCC; 32]),
            });
        }
        // Add the same object's confirmation INSIDE the window (matching payload).
        main_lane.push(MainLaneTx {
            round: lineage_round + 1,
            object: tx.object,
            payload_digest: tx.payload_digest,
            lineage: CertHash([0xDD; 32]),
        });

        prop_assert!(is_main_lane_consistent(&cert, &main_lane));
    }

    /// The fast-path quorum size formula matches paper §6.4 exactly.
    #[test]
    fn fast_path_quorum_matches_paper(n in 1u32..=200) {
        let q = fast_path_quorum_size(n);
        let expected = ((2 * n).div_ceil(3) + 1).min(n);
        prop_assert_eq!(q, expected);
    }

    /// A conflicting tx (same object, different payload) inside the
    /// `(R, R+K]` window breaks binding.
    #[test]
    fn conflict_within_k_rounds_breaks_binding(
        n_authorities in 3u32..=50,
        object_seed in any::<u8>(),
        payload_seed in any::<u8>(),
        lineage_round in 0u64..=100,
        conflict_offset in 1u32..=FAST_PATH_CONFIRMATION_K,
    ) {
        // Avoid the case where the "conflicting" payload happens to
        // collide with the certified payload — proptest reject.
        let conflict_payload = payload_seed.wrapping_add(1);
        prop_assume!(conflict_payload != payload_seed);

        let q = fast_path_quorum_size(n_authorities);
        let signers: BTreeSet<AuthorityId> = (0..q).collect();
        let tx = build_tx(object_seed, lineage_round, payload_seed);
        let cert = certify(tx.clone(), signers, n_authorities).unwrap();

        let main_lane = vec![MainLaneTx {
            round: lineage_round + conflict_offset as u64,
            object: tx.object,
            payload_digest: [conflict_payload; 32],
            lineage: CertHash([0xCC; 32]),
        }];

        prop_assert!(!is_main_lane_consistent(&cert, &main_lane));
    }

    /// The same conflict strictly past `R + K` does NOT signal
    /// inconsistency. The main-lane Mysticeti-C commit rule handles it.
    #[test]
    fn conflict_beyond_k_rounds_preserves_binding(
        n_authorities in 3u32..=50,
        object_seed in any::<u8>(),
        payload_seed in any::<u8>(),
        lineage_round in 0u64..=100,
        beyond_offset in (FAST_PATH_CONFIRMATION_K + 1)..=20,
    ) {
        let conflict_payload = payload_seed.wrapping_add(1);
        prop_assume!(conflict_payload != payload_seed);

        let q = fast_path_quorum_size(n_authorities);
        let signers: BTreeSet<AuthorityId> = (0..q).collect();
        let tx = build_tx(object_seed, lineage_round, payload_seed);
        let cert = certify(tx.clone(), signers, n_authorities).unwrap();

        let main_lane = vec![MainLaneTx {
            round: lineage_round + beyond_offset as u64,
            object: tx.object,
            payload_digest: [conflict_payload; 32],
            lineage: CertHash([0xCC; 32]),
        }];

        prop_assert!(is_main_lane_consistent(&cert, &main_lane));
    }
}
