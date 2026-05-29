//! Certificate signing exit-gate property tests (Task 1 / C4).
//!
//! Exit gate: `cert_signature_roundtrips` — for any authority id and
//! payload, a certificate signed by the correct ML-DSA-65 key verifies
//! against the matching public key in the Authority Registry.
//!
//! Supporting properties:
//!
//! - `wrong_key_rejects` — a cert signed by authority A's key does not
//!   verify against authority B's key.
//! - `unsigned_cert_rejects` — a cert with an empty signature is
//!   rejected by `verify_cert_signature`.
//! - `tampered_cert_rejects` — flipping a byte in the payload after
//!   signing invalidates the signature.
//!
//! Run at default 256 cases (aligned with the rest of the proptest
//! suite); sprint close runs `PROPTEST_CASES=10000 cargo test
//! -p gsx-node --release`.

use gsx_consensus::Certificate;
use gsx_node::{seed_registry, sign_cert, verify_cert_signature};
use proptest::prelude::*;

const NET: &str = "test";

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 16,
        .. ProptestConfig::default()
    })]

    /// A correctly signed certificate verifies against the registry.
    #[test]
    fn cert_signature_roundtrips(
        author in 0u32..4,
        payload_byte in 0u8..=255,
    ) {
        let n = 4u32;
        let (registry, sks) = seed_registry(n);
        let mut cert = Certificate::genesis(author, [payload_byte; 32]);
        sign_cert(&mut cert, &sks[author as usize], NET);

        // Must verify successfully.
        prop_assert!(verify_cert_signature(&cert, &registry, NET).is_ok());
    }

    /// A cert signed by authority A must not verify as authority B.
    #[test]
    fn wrong_key_rejects(
        author in 0u32..4,
        impersonator in 0u32..4,
        payload_byte in 0u8..=255,
    ) {
        prop_assume!(author != impersonator);
        let n = 4u32;
        let (registry, sks) = seed_registry(n);

        // Sign with impersonator's key but claim to be `author`.
        let mut cert = Certificate::genesis(author, [payload_byte; 32]);
        sign_cert(&mut cert, &sks[impersonator as usize], NET);

        // Must fail — the signature doesn't match author's public key.
        prop_assert!(verify_cert_signature(&cert, &registry, NET).is_err());
    }

    /// An unsigned certificate (empty signature) is rejected.
    #[test]
    fn unsigned_cert_rejects(
        author in 0u32..4,
        payload_byte in 0u8..=255,
    ) {
        let n = 4u32;
        let (registry, _sks) = seed_registry(n);
        let cert = Certificate::genesis(author, [payload_byte; 32]);

        // Signature is empty — must fail.
        prop_assert!(verify_cert_signature(&cert, &registry, NET).is_err());
    }

    /// Tampering with the payload after signing invalidates the signature.
    #[test]
    fn tampered_cert_rejects(
        author in 0u32..4,
        payload_byte in 0u8..=254,
    ) {
        let n = 4u32;
        let (registry, sks) = seed_registry(n);
        let mut cert = Certificate::genesis(author, [payload_byte; 32]);
        sign_cert(&mut cert, &sks[author as usize], NET);

        // Verify original works.
        prop_assert!(verify_cert_signature(&cert, &registry, NET).is_ok());

        // Tamper with the payload — flip one byte.
        cert.payload_digest[0] = payload_byte.wrapping_add(1);

        // Must fail — hash changed, signature is now invalid.
        prop_assert!(verify_cert_signature(&cert, &registry, NET).is_err());
    }
}
