//! Vote signing exit-gate property tests (Task 2 / C5).
//!
//! Exit gate: `vote_signature_roundtrips` — for any validator id and
//! candidate hash, a vote signed by the correct ML-DSA-65 key verifies
//! against the matching public key in the Validator Registry.
//!
//! Supporting properties:
//!
//! - `wrong_key_rejects` — a vote signed by validator A's key does not
//!   verify against validator B's key.
//! - `unsigned_vote_rejects` — a vote with an empty signature is
//!   rejected by `verify_vote_signature`.
//! - `tampered_vote_rejects` — flipping a byte in the candidate hash
//!   after signing invalidates the signature.
//!
//! Run at default 256 cases (aligned with the rest of the proptest
//! suite); sprint close runs `PROPTEST_CASES=10000 cargo test
//! -p gsx-node --release`.

use gsx_consensus::{CertHash, Vote};
use gsx_crypto::mldsa;
use gsx_node::{sign_vote, verify_vote_signature};
use gsx_validator::{ValidatorMember, ValidatorRegistry, VALIDATOR_STAKE_THRESHOLD_GSX};
use proptest::prelude::*;

const NET: &str = "test";

/// Build a Validator Registry with `n` members using real ML-DSA-65
/// keypairs. Returns the registry and the secret keys.
fn seed_validator_registry(n: u32) -> (ValidatorRegistry, Vec<mldsa::SecretKey>) {
    let mut registry = ValidatorRegistry::new();
    let mut sks = Vec::with_capacity(n as usize);
    for i in 0..n {
        let (pk, sk) = mldsa::keypair();
        registry
            .admit(ValidatorMember {
                id: i,
                stake_gsx: VALIDATOR_STAKE_THRESHOLD_GSX,
                public_key_bytes: pk.as_bytes().to_vec(),
            })
            .expect("seed");
        sks.push(sk);
    }
    (registry, sks)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 16,
        .. ProptestConfig::default()
    })]

    /// A correctly signed vote verifies against the registry.
    #[test]
    fn vote_signature_roundtrips(
        voter in 0u32..4,
        candidate_byte in 0u8..=255,
    ) {
        let n = 4u32;
        let (registry, sks) = seed_validator_registry(n);
        let mut vote = Vote {
            validator: voter,
            candidate: CertHash::from([candidate_byte; 32]),
            signature: vec![],
        };
        sign_vote(&mut vote, &sks[voter as usize], NET);

        // Must verify successfully.
        prop_assert!(verify_vote_signature(&vote, &registry, NET).is_ok());
    }

    /// A vote signed by validator A must not verify as validator B.
    #[test]
    fn wrong_key_rejects(
        voter in 0u32..4,
        impersonator in 0u32..4,
        candidate_byte in 0u8..=255,
    ) {
        prop_assume!(voter != impersonator);
        let n = 4u32;
        let (registry, sks) = seed_validator_registry(n);

        // Sign with impersonator's key but claim to be `voter`.
        let mut vote = Vote {
            validator: voter,
            candidate: CertHash::from([candidate_byte; 32]),
            signature: vec![],
        };
        sign_vote(&mut vote, &sks[impersonator as usize], NET);

        // Must fail — the signature doesn't match voter's public key.
        prop_assert!(verify_vote_signature(&vote, &registry, NET).is_err());
    }

    /// An unsigned vote (empty signature) is rejected.
    #[test]
    fn unsigned_vote_rejects(
        voter in 0u32..4,
        candidate_byte in 0u8..=255,
    ) {
        let n = 4u32;
        let (registry, _sks) = seed_validator_registry(n);
        let vote = Vote {
            validator: voter,
            candidate: CertHash::from([candidate_byte; 32]),
            signature: vec![],
        };

        // Signature is empty — must fail.
        prop_assert!(verify_vote_signature(&vote, &registry, NET).is_err());
    }

    /// Tampering with the candidate hash after signing invalidates the signature.
    #[test]
    fn tampered_vote_rejects(
        voter in 0u32..4,
        candidate_byte in 0u8..254,
    ) {
        let n = 4u32;
        let (registry, sks) = seed_validator_registry(n);
        let mut vote = Vote {
            validator: voter,
            candidate: CertHash::from([candidate_byte; 32]),
            signature: vec![],
        };
        sign_vote(&mut vote, &sks[voter as usize], NET);

        // Verify original works.
        prop_assert!(verify_vote_signature(&vote, &registry, NET).is_ok());

        // Tamper with the candidate — flip one byte. CertHash's inner array
        // is private (constructors migrated to ::from), so round-trip through
        // the byte array to mutate.
        let mut tampered: [u8; 32] = vote.candidate.into();
        tampered[0] = candidate_byte.wrapping_add(1);
        vote.candidate = CertHash::from(tampered);

        // Must fail — digest changed, signature is now invalid.
        prop_assert!(verify_vote_signature(&vote, &registry, NET).is_err());
    }
}
