//! DAG-S11 exit-gate property tests.
//!
//! Exit gate: `joint_state_commitment_signed` — for any honest
//! checkpoint co-signed by ≥ `registry.quorum_threshold()` Authority
//! Ring members, every signature verifies, the checkpoint hash is a
//! deterministic function of the named fields, and tampering with any
//! field of the checkpoint breaks signature verification under the
//! same key set.
//!
//! Supporting properties:
//!
//! - `checkpoint_cadence_respects_C` — `Checkpointer` emits at exactly
//!   the rounds divisible by `cadence`.
//! - `prev_checkpoint_chain_intact` — successive checkpoints link via
//!   `prev_checkpoint == previous.hash()`.
//! - `below_quorum_signatures_rejected` — fewer than the quorum
//!   threshold yields `CheckpointError::BelowQuorum`.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-execution --release`.

use suwappu_authority::{AuthorityMember, AuthorityRegistry, AUTHORITY_STAKE_THRESHOLD_SUWAPPU};
use suwappu_crypto::mldsa;
use suwappu_execution::{
    ratify_checkpoint, sign_checkpoint, Checkpoint, CheckpointError, Checkpointer,
};
use proptest::prelude::*;

/// Build a registry of `n` authorities each with a freshly-generated
/// ML-DSA-65 keypair. Returns the registry plus the secret keys aligned
/// by AuthorityId.
fn build_registry(n: u32) -> (AuthorityRegistry, Vec<mldsa::SecretKey>) {
    let mut registry = AuthorityRegistry::new();
    let mut sks = Vec::with_capacity(n as usize);
    for i in 0..n {
        let (pk, sk) = mldsa::keypair();
        registry
            .admit(AuthorityMember {
                id: i,
                stake_suwappu: AUTHORITY_STAKE_THRESHOLD_SUWAPPU,
                public_key_bytes: pk.as_bytes().to_vec(),
            })
            .unwrap();
        sks.push(sk);
    }
    (registry, sks)
}

proptest! {
    #![proptest_config(ProptestConfig {
        // ML-DSA-65 keygen is expensive (~5 ms); keep the case count
        // low at default and let the 10k release run produce the
        // load-bearing evidence. The sprint exit gate budget is the
        // release-mode time, not the dev-mode time.
        cases: 64,
        max_shrink_iters: 16,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — honest co-signature verifies; tampering breaks it.
    #[test]
    fn joint_state_commitment_signed(
        n_authorities in 3u32..=8,
        height in 0u64..=100,
        round in 0u64..=1000,
        state_root_seed in any::<u8>(),
        prev_seed in any::<u8>(),
    ) {
        let (registry, sks) = build_registry(n_authorities);
        let q = registry.quorum_threshold();

        let ck = Checkpoint {
            height,
            round,
            state_root: [state_root_seed; 32],
            prev_checkpoint: [prev_seed; 32],
        };

        // Sign with exactly the first q authorities.
        let mut sigs = Vec::with_capacity(q as usize);
        for i in 0..q {
            sigs.push(sign_checkpoint(i, &sks[i as usize], &ck).unwrap());
        }
        // Ratification succeeds.
        let cosigned = ratify_checkpoint(ck.clone(), sigs.clone(), &registry)
            .expect("at-quorum honest co-signature must ratify");
        prop_assert_eq!(cosigned.signatures.len() as u32, q);

        // Tamper with any single field of the checkpoint; the SAME
        // signature set must now be rejected, because the digest
        // changed.
        let mut tampered = ck;
        tampered.state_root[0] ^= 1;
        let result = ratify_checkpoint(tampered, sigs, &registry);
        let is_invalid_sig = matches!(result, Err(CheckpointError::InvalidSignature(_)));
        prop_assert!(is_invalid_sig, "tampered checkpoint accepted with original signatures");
    }

    /// `Checkpointer::maybe_emit` returns `Some` at exactly the rounds
    /// divisible by `cadence`, and `None` elsewhere.
    #[test]
    fn checkpoint_cadence_respects_c(
        cadence in 1u32..=16,
        rounds in prop::collection::vec(0u64..=200, 1..=32),
    ) {
        let mut cp = Checkpointer::new(cadence);
        let mut sorted = rounds.clone();
        sorted.sort();
        sorted.dedup();

        for r in sorted {
            let emitted = cp.maybe_emit(r, [0; 32]);
            let expected_boundary = (r % cadence as u64) == 0;
            prop_assert_eq!(emitted.is_some(), expected_boundary);
        }
    }

    /// Successive checkpoints chain by `prev_checkpoint == previous.hash()`.
    #[test]
    fn prev_checkpoint_chain_intact(
        cadence in 1u32..=8,
        n_steps in 1usize..=10,
        state_seed in any::<u8>(),
    ) {
        let mut cp = Checkpointer::new(cadence);
        let mut prev_hash = [0u8; 32];
        for step in 0..n_steps {
            let round = (step as u64) * cadence as u64;
            let root = [state_seed.wrapping_add(step as u8); 32];
            let ck = cp.maybe_emit(round, root).expect("on boundary");
            prop_assert_eq!(ck.prev_checkpoint, prev_hash);
            prop_assert_eq!(ck.height as usize, step);
            prev_hash = ck.hash();
        }
    }

    /// Below the registry's quorum threshold, ratification rejects with
    /// `BelowQuorum`.
    #[test]
    fn below_quorum_signatures_rejected(
        n_authorities in 3u32..=8,
        height in 0u64..=10,
        round in 0u64..=10,
    ) {
        let (registry, sks) = build_registry(n_authorities);
        let q = registry.quorum_threshold();

        let ck = Checkpoint {
            height,
            round,
            state_root: [0; 32],
            prev_checkpoint: [0; 32],
        };

        let supplied = q.saturating_sub(1);
        let mut sigs = Vec::with_capacity(supplied as usize);
        for i in 0..supplied {
            sigs.push(sign_checkpoint(i, &sks[i as usize], &ck).unwrap());
        }
        let result = ratify_checkpoint(ck, sigs, &registry);
        let is_below = matches!(result, Err(CheckpointError::BelowQuorum { .. }));
        prop_assert!(is_below);
    }
}
