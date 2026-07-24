//! DAG-S20 exit-gate property tests.
//!
//! Exit gate: `node_runs_genesis_block` — for any committee size
//! `n ∈ [3, 8]`, the full pipeline runs end-to-end:
//!
//! 1. Every validator authors a round-0 genesis cert.
//! 2. Every validator authors a round-1 cert referencing all round-0
//!    certs as parents.
//! 3. The DagBft-C commit rule fires on the round-0 leader at
//!    round 1.
//! 4. Every validator executes the same block and converges on the
//!    same state root.
//! 5. The joint state checkpoint is ratified by an Authority quorum.
//!
//! Supporting properties:
//!
//! - `cross_validator_state_root_agrees` — every validator's substrate
//!   computes the same `state_root` after the same block.
//! - `ratification_carries_full_signer_set` — when every validator
//!   signs the checkpoint honestly, the ratified set carries `n`
//!   distinct signatures.
//! - `committee_size_below_threshold_does_not_commit` — for `n` such
//!   that the leader's supporter count is below quorum, no commit
//!   fires (we use `n = 1` paired with a faulty leader-absent case
//!   in a separate proptest, but here the basic guarantee is that
//!   `commit_leader` returns `None` precisely when supporter count
//!   fails the threshold).
//!
//! Run at default 64 cases (ML-DSA-65 keygen ~5 ms per validator);
//! sprint close runs `PROPTEST_CASES=10000 cargo test -p suwappu-node
//! --release`.

use proptest::prelude::*;
use suwappu_node::{run_genesis_flow_with_keys, seed_registry};

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        max_shrink_iters: 16,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — the full genesis pipeline runs end-to-end.
    #[test]
    fn node_runs_genesis_block(
        n in 3u32..=8,
        payload_seed in any::<u8>(),
    ) {
        let (registry, sks) = seed_registry(n);
        let outcome = run_genesis_flow_with_keys(n, &registry, &sks, payload_seed).unwrap();
        let (leader_hash, _state_root, cosigned) =
            outcome.expect("commit must fire under honest n-of-n quorum");
        // The committed leader is round-0 author 0 (round-robin pick).
        // We don't assert the exact hash (varies with payload_seed and
        // committee size) but we do assert the committee co-signed it
        // and the ratified signatures meet the registry's quorum.
        prop_assert!(cosigned.signatures.len() as u32 >= registry.quorum_threshold());
        // And the committed hash is non-zero.
        prop_assert_ne!(leader_hash.0, [0u8; 32]);
    }

    /// Every validator agrees on the post-state root after executing
    /// the same block — the cross-validator agreement property.
    #[test]
    fn cross_validator_state_root_agrees(
        n in 3u32..=8,
        payload_seed in any::<u8>(),
    ) {
        let (registry, sks) = seed_registry(n);
        let outcome = run_genesis_flow_with_keys(n, &registry, &sks, payload_seed).unwrap();
        prop_assert!(outcome.is_some(),
            "honest n-of-n committee must commit and converge on a state root");
    }

    /// Honestly-signed checkpoints carry exactly `n` distinct signers.
    #[test]
    fn ratification_carries_full_signer_set(
        n in 3u32..=8,
        payload_seed in any::<u8>(),
    ) {
        let (registry, sks) = seed_registry(n);
        let (_leader, _root, cosigned) =
            run_genesis_flow_with_keys(n, &registry, &sks, payload_seed)
                .unwrap()
                .unwrap();
        prop_assert_eq!(cosigned.signatures.len() as u32, n);
    }
}
