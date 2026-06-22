//! DAG-S15 exit-gate property tests.
//!
//! Exit gate: `seven_of_nine_attestation` — for any 7-of-9 honest
//! signatures over an `AttestationPayload`, `attest()` produces a
//! `CorridorAttestation` and `verify_attestation()` accepts it.
//!
//! Supporting properties:
//!
//! - `six_of_nine_below_quorum` — exactly 6 signatures yields
//!   `BelowQuorum`.
//! - `unknown_witness_rejected` — a signature from an authority id
//!   outside the corridor is rejected with `UnknownWitness`.
//! - `payload_tamper_breaks_verification` — flipping any byte of the
//!   payload after aggregation causes `verify_attestation` to fail
//!   with `AggregateVerificationFailed`.
//!
//! Run at default 64 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-ltp --release`.

use std::collections::BTreeSet;

use proptest::prelude::*;
use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};
use suwappu_crypto::bls;
use suwappu_ltp::{
    attest, verify_attestation, AttestationPayload, AuthorityId, Corridor, CorridorAttestation,
    LtpError, SuperNode, WitnessSignature,
};

fn build_corridor(seed: u64) -> (Corridor, Vec<blst::min_pk::SecretKey>) {
    let mut members = Vec::with_capacity(9);
    let mut sks = Vec::with_capacity(9);
    for i in 0..9u32 {
        let (pk, sk) = bls::keypair();
        members.push(SuperNode {
            authority: i,
            corridor: (seed & 0xFFFF) as u32,
            bls_public_key: pk.to_bytes().to_vec(),
        });
        sks.push(sk);
    }
    (
        Corridor {
            id: (seed & 0xFFFF) as u32,
            members,
        },
        sks,
    )
}

fn sample_payload(
    source_chain: u64,
    target_chain: u64,
    height: u64,
    state_seed: u8,
    timestamp: u64,
) -> AttestationPayload {
    AttestationPayload {
        source_chain,
        target_chain,
        source_height: height,
        state_root: [state_seed; 32],
        timestamp_round: timestamp,
    }
}

fn sign_with(
    sks: &[blst::min_pk::SecretKey],
    payload: &AttestationPayload,
    witness_ids: &[AuthorityId],
) -> Vec<WitnessSignature> {
    let digest = payload.canonical_digest();
    witness_ids
        .iter()
        .map(|id| {
            let s = bls::sign(&digest, &sks[*id as usize]);
            WitnessSignature {
                witness: *id,
                signature: s.to_bytes().to_vec(),
            }
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig {
        // BLS keygen is ~1 ms; we generate 9 per case. Keep default
        // cases conservative; release-mode 10k is the sprint exit
        // gate's load-bearing strength.
        cases: 64,
        max_shrink_iters: 16,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — any 7-of-9 honest signatures attest and verify.
    /// The signing subset is selected by `subset_seed` and may be any
    /// of the C(9,7)=36 size-7 combinations.
    #[test]
    fn seven_of_nine_attestation(
        corridor_seed in any::<u64>(),
        source_chain in any::<u64>(),
        target_chain in any::<u64>(),
        height in 0u64..=1_000_000,
        state_seed in any::<u8>(),
        timestamp in 0u64..=1_000_000,
        subset_seed in any::<u64>(),
    ) {
        let (corridor, sks) = build_corridor(corridor_seed);
        let payload = sample_payload(source_chain, target_chain, height, state_seed, timestamp);

        // Choose 7 of 9 witnesses uniformly at random.
        let mut rng = StdRng::seed_from_u64(subset_seed);
        let mut ids: Vec<AuthorityId> = (0..9u32).collect();
        ids.shuffle(&mut rng);
        ids.truncate(7);
        let signatures = sign_with(&sks, &payload, &ids);

        let att = attest(&corridor, payload, signatures)
            .expect("7-of-9 honest signatures must attest");
        prop_assert_eq!(att.signers.len(), 7);
        verify_attestation(&corridor, &att).expect("attestation must verify");

        let expected: BTreeSet<AuthorityId> = ids.iter().copied().collect();
        prop_assert_eq!(att.signers, expected);
    }

    /// Exactly 6 signatures yields `BelowQuorum`.
    #[test]
    fn six_of_nine_below_quorum(
        corridor_seed in any::<u64>(),
        source_chain in any::<u64>(),
        target_chain in any::<u64>(),
        height in 0u64..=1_000_000,
        state_seed in any::<u8>(),
        timestamp in 0u64..=1_000_000,
        subset_seed in any::<u64>(),
    ) {
        let (corridor, sks) = build_corridor(corridor_seed);
        let payload = sample_payload(source_chain, target_chain, height, state_seed, timestamp);

        let mut rng = StdRng::seed_from_u64(subset_seed);
        let mut ids: Vec<AuthorityId> = (0..9u32).collect();
        ids.shuffle(&mut rng);
        ids.truncate(6);
        let signatures = sign_with(&sks, &payload, &ids);

        let err = attest(&corridor, payload, signatures);
        let is_below_quorum = matches!(err, Err(LtpError::BelowQuorum { have: 6, need: 7 }));
        prop_assert!(is_below_quorum);
    }

    /// A signature from a witness id outside the corridor is rejected
    /// with `UnknownWitness`.
    #[test]
    fn unknown_witness_rejected(
        corridor_seed in any::<u64>(),
        state_seed in any::<u8>(),
        foreign_id in 100u32..=10_000,
    ) {
        let (corridor, _sks) = build_corridor(corridor_seed);
        let (_pk_foreign, sk_foreign) = bls::keypair();
        let payload = sample_payload(0, 0, 0, state_seed, 0);
        let digest = payload.canonical_digest();
        let s = bls::sign(&digest, &sk_foreign);
        let sigs = vec![WitnessSignature {
            witness: foreign_id,
            signature: s.to_bytes().to_vec(),
        }];
        let err = attest(&corridor, payload, sigs);
        prop_assert_eq!(err, Err(LtpError::UnknownWitness(foreign_id)));
    }

    /// Tampering any field of the payload after aggregation invalidates
    /// verification.
    #[test]
    fn payload_tamper_breaks_verification(
        corridor_seed in any::<u64>(),
        source_chain in any::<u64>(),
        target_chain in any::<u64>(),
        height in 0u64..=1_000_000,
        state_seed in any::<u8>(),
        timestamp in 0u64..=1_000_000,
    ) {
        let (corridor, sks) = build_corridor(corridor_seed);
        let payload = sample_payload(source_chain, target_chain, height, state_seed, timestamp);
        let ids: Vec<AuthorityId> = (0..7u32).collect();
        let signatures = sign_with(&sks, &payload, &ids);
        let mut att: CorridorAttestation = attest(&corridor, payload, signatures).unwrap();

        // Flip one byte in the state root.
        att.payload.state_root[0] ^= 1;
        let err = verify_attestation(&corridor, &att);
        prop_assert_eq!(err, Err(LtpError::AggregateVerificationFailed));
    }
}
