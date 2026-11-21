//! DAG-S34.4 exit gate (IQ-008 D5, invariant candidate I-CK1): checkpoint
//! chain soundness.
//!
//! `verify_checkpoint_chain` accepts a chain iff every link is co-signed by
//! a quorum of the committee established by its predecessor (the genesis
//! committee for the first link) and binds the committee it establishes.
//! The honest construction is accepted across arbitrary committee
//! changes; any of the following is rejected: a link signed below
//! quorum, a link signed by a committee other than the one in force, a
//! forged signature, a `registry_root` that does not bind the supplied
//! committee, a non-monotone height or round, a broken `prev_checkpoint`
//! link between consecutive heights.
//!
//! Run at default 32 cases under CI (ML-DSA-65 keygen dominates); sprint
//! close runs `PROPTEST_CASES=10000 cargo test -p suwappu-execution --release`.

use proptest::prelude::*;
use suwappu_authority::{AuthorityMember, AuthorityRegistry, AUTHORITY_STAKE_THRESHOLD_SUWAPPU};
use suwappu_crypto::mldsa;
use suwappu_execution::{
    sign_checkpoint, verify_checkpoint_chain, ChainError, ChainLink, Checkpoint, CheckpointError,
};

struct Committee {
    registry: AuthorityRegistry,
    sks: Vec<(u32, mldsa::SecretKey)>,
}

fn committee(ids: &[u32]) -> Committee {
    let mut registry = AuthorityRegistry::new();
    let mut sks = Vec::new();
    for &i in ids {
        let (pk, sk) = mldsa::keypair();
        registry
            .admit(AuthorityMember {
                id: i,
                stake_suwappu: AUTHORITY_STAKE_THRESHOLD_SUWAPPU,
                public_key_bytes: pk.as_bytes().to_vec(),
            })
            .unwrap();
        sks.push((i, sk));
    }
    Committee { registry, sks }
}

/// Deterministic stand-in for the node's registry root: hash of the
/// member ids + keys. The verifier treats it as opaque bytes, so any
/// injective function of the committee is a faithful model.
fn root_of(reg: &AuthorityRegistry) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    for m in reg.members() {
        h.update(&m.id.to_be_bytes());
        h.update(&m.public_key_bytes);
    }
    *h.finalize().as_bytes()
}

/// Build an honest chain: link `i` is signed by committee `i` (the
/// genesis committee for i = 0) and establishes committee `i + 1`.
fn honest_chain(committees: &[Committee], signers_per_link: &[usize]) -> Vec<ChainLink> {
    let mut links = Vec::new();
    let mut prev = [0u8; 32];
    for i in 0..committees.len() - 1 {
        let next = &committees[i + 1];
        let ck = Checkpoint {
            height: i as u64,
            round: (i as u64 + 1) * 100,
            state_root: [i as u8 + 1; 32],
            prev_checkpoint: prev,
            registry_root: root_of(&next.registry),
            snapshot_root: [0; 32],
        };
        prev = ck.hash();
        let signatures = committees[i]
            .sks
            .iter()
            .take(signers_per_link[i])
            .map(|(id, sk)| sign_checkpoint(*id, sk, &ck).unwrap())
            .collect();
        links.push(ChainLink {
            checkpoint: ck,
            signatures,
            next_committee: next.registry.clone(),
            next_registry_root: root_of(&next.registry),
        });
    }
    links
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(32),
        max_shrink_iters: 8,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE (I-CK1): an honestly co-signed chain across committee
    /// changes verifies; every single-point corruption is rejected.
    #[test]
    fn chain_accepts_honest_and_rejects_corruption(
        sizes in prop::collection::vec(1u32..=4, 2..=4),
        seed in any::<u32>(),
        corrupt in 0u8..=6,
    ) {
        // Committee i has `sizes[i]` members with ids disjoint per step so
        // that a signature from the wrong committee is never accidentally
        // valid under the right one.
        let committees: Vec<Committee> = sizes
            .iter()
            .enumerate()
            .map(|(step, n)| {
                let base = (step as u32) * 10;
                committee(&(base..base + n).collect::<Vec<_>>())
            })
            .collect();
        let quorums: Vec<usize> = committees.iter().map(|c| c.registry.quorum_threshold() as usize).collect();
        let links = honest_chain(&committees, &quorums);
        let genesis = &committees[0].registry;

        let verified = verify_checkpoint_chain(genesis, &links).expect("honest chain must verify");
        prop_assert_eq!(verified.len(), links.len());
        for (v, l) in verified.iter().zip(&links) {
            prop_assert_eq!(&v.checkpoint, &l.checkpoint);
            prop_assert!(v.signatures.len() >= l.next_committee.len().min(v.signatures.len()));
        }
        // An empty chain is trivially fine.
        prop_assert!(verify_checkpoint_chain(genesis, &[]).unwrap().is_empty());

        let target = (seed as usize) % links.len();
        let mut bad = links.clone();
        match corrupt {
            0 => {
                // Below quorum on one link.
                let q = quorums[target];
                bad[target].signatures.truncate(q - 1);
                let err = verify_checkpoint_chain(genesis, &bad).unwrap_err();
                prop_assert!(matches!(err, ChainError::Link { index, source: CheckpointError::BelowQuorum { .. } } if index == target), "{err:?}");
            }
            1 => {
                // Signed by the committee it establishes instead of the
                // one in force (a would-be self-appointing committee).
                let next = &committees[target + 1];
                bad[target].signatures = next
                    .sks
                    .iter()
                    .map(|(id, sk)| sign_checkpoint(*id, sk, &bad[target].checkpoint).unwrap())
                    .collect();
                let err = verify_checkpoint_chain(genesis, &bad).unwrap_err();
                prop_assert!(matches!(err, ChainError::Link { index, .. } if index == target), "{err:?}");
            }
            2 => {
                // Forged signature bytes.
                let sig = &mut bad[target].signatures[0].signature;
                sig[0] ^= 0x5A;
                let err = verify_checkpoint_chain(genesis, &bad).unwrap_err();
                prop_assert!(matches!(err, ChainError::Link { index, source: CheckpointError::InvalidSignature(_) } if index == target), "{err:?}");
            }
            3 => {
                // Registry root does not bind the supplied committee: swap
                // in a different committee without re-signing.
                let other = committee(&[99]);
                bad[target].next_committee = other.registry.clone();
                bad[target].next_registry_root = root_of(&other.registry);
                let err = verify_checkpoint_chain(genesis, &bad).unwrap_err();
                prop_assert!(matches!(err, ChainError::RegistryRootMismatch { index } if index == target), "{err:?}");
            }
            4 => {
                // Registry root changed in the checkpoint itself: the
                // signatures no longer cover it.
                bad[target].checkpoint.registry_root[0] ^= 1;
                bad[target].next_registry_root = bad[target].checkpoint.registry_root;
                let err = verify_checkpoint_chain(genesis, &bad).unwrap_err();
                prop_assert!(matches!(err, ChainError::Link { index, source: CheckpointError::InvalidSignature(_) } if index == target), "{err:?}");
            }
            5 => {
                // Broken prev-hash link between consecutive heights: the
                // honest chain is contiguous, so flipping a link's
                // `prev_checkpoint` (and re-signing it honestly, so the
                // signatures are not what fails) must be rejected.
                bad[target].checkpoint.prev_checkpoint[0] ^= 1;
                bad[target].signatures = committees[target]
                    .sks
                    .iter()
                    .take(quorums[target])
                    .map(|(id, sk)| sign_checkpoint(*id, sk, &bad[target].checkpoint).unwrap())
                    .collect();
                let err = verify_checkpoint_chain(genesis, &bad).unwrap_err();
                prop_assert!(matches!(err, ChainError::BrokenLink { index } if index == target), "{err:?}");
            }
            _ => {
                // Non-monotone: replay an earlier link after a later one.
                if links.len() >= 2 {
                    let mut replay = links.clone();
                    let first = replay[0].clone();
                    replay.push(first);
                    let err = verify_checkpoint_chain(genesis, &replay).unwrap_err();
                    prop_assert!(matches!(err, ChainError::NotMonotone { .. }), "{err:?}");
                }
            }
        }
    }
}
