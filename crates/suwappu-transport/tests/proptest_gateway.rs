//! DAG-S19 exit-gate property tests.
//!
//! Exit gate: `gateway_fallback_correctness` — for any honest IP
//! request encapsulated under a valid SCION path and signed by the
//! gateway, `verify_response` accepts. Tampering with the envelope,
//! the response, or substituting a foreign signer all break
//! verification.
//!
//! Supporting properties:
//!
//! - `tampered_response_payload_rejected` — any byte flip in
//!   `response.response_payload` breaks signature verification.
//! - `envelope_tamper_rejected` — flipping any byte in the envelope
//!   (after the gateway signs) causes `DigestMismatch` because the
//!   `request_digest` in the response no longer matches.
//! - `foreign_signer_rejected` — a response signed by a key other
//!   than `config.gateway_pubkey` fails verification.
//!
//! Run at default 64 cases under CI (ML-DSA-65 keygen ~5 ms per case);
//! sprint close runs `PROPTEST_CASES=10000 cargo test -p suwappu-transport
//! --release`.

use std::collections::BTreeMap;

use suwappu_crypto::mldsa;
use suwappu_transport::{
    seal_path, sign_response, verify_response, GatewayConfig, GatewayEnvelope, GatewayError,
    HopField, IpPacket, TrustRootConfig,
};
use proptest::prelude::*;

fn build_config(seed: u64) -> (GatewayConfig, mldsa::SecretKey) {
    let mut as_keys = BTreeMap::new();
    let mut key = [0u8; 32];
    key[0..8].copy_from_slice(&seed.to_be_bytes());
    as_keys.insert(10u32, key);
    let trc = TrustRootConfig {
        isd: 1,
        version: 1,
        as_keys,
        valid_until: 1_000_000,
    };
    let (pk, sk) = mldsa::keypair();
    (
        GatewayConfig {
            gateway_isd: 1,
            gateway_as: 10,
            gateway_pubkey: pk.as_bytes().to_vec(),
            trc,
        },
        sk,
    )
}

fn build_envelope(config: &GatewayConfig, payload: Vec<u8>, created_at: u64) -> GatewayEnvelope {
    let hops = vec![HopField {
        isd_as: (1, 10),
        ingress_iface: 1,
        egress_iface: 2,
        expiration_round: 1_000_000,
        mac: [0u8; 16],
    }];
    let path = seal_path(1, created_at, hops, &config.trc).unwrap();
    GatewayEnvelope {
        ip_packet: IpPacket {
            source_ip: [10; 16],
            dest_ip: [20; 16],
            payload,
        },
        scion_path: path,
        created_at,
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        max_shrink_iters: 16,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — honest sign + verify accepts.
    #[test]
    fn gateway_fallback_correctness(
        gateway_seed in any::<u64>(),
        request_payload in prop::collection::vec(any::<u8>(), 0..=512),
        response_payload in prop::collection::vec(any::<u8>(), 0..=512),
        created_at in 0u64..=10_000,
        sign_at in 0u64..=10_000,
    ) {
        let (config, sk) = build_config(gateway_seed);
        let envelope = build_envelope(&config, request_payload, created_at);
        let response = sign_response(&config, &sk, &envelope, response_payload, sign_at).unwrap();
        verify_response(&config, &envelope, &response, sign_at).unwrap();
    }

    /// Flipping any byte of the response payload breaks the signature.
    #[test]
    fn tampered_response_payload_rejected(
        gateway_seed in any::<u64>(),
        request_payload in prop::collection::vec(any::<u8>(), 0..=256),
        response_payload in prop::collection::vec(any::<u8>(), 1..=256),
        flip_idx in 0usize..=255,
    ) {
        let (config, sk) = build_config(gateway_seed);
        let envelope = build_envelope(&config, request_payload, 100);
        let mut response = sign_response(
            &config, &sk, &envelope, response_payload.clone(), 200,
        ).unwrap();
        let idx = flip_idx % response.response_payload.len();
        response.response_payload[idx] ^= 1;
        let err = verify_response(&config, &envelope, &response, 200);
        prop_assert_eq!(err, Err(GatewayError::InvalidSignature));
    }

    /// Tampering the envelope after the gateway signs causes
    /// `DigestMismatch`.
    #[test]
    fn envelope_tamper_rejected(
        gateway_seed in any::<u64>(),
        request_payload in prop::collection::vec(any::<u8>(), 1..=256),
        response_payload in prop::collection::vec(any::<u8>(), 0..=256),
        flip_idx in 0usize..=255,
    ) {
        let (config, sk) = build_config(gateway_seed);
        let envelope = build_envelope(&config, request_payload, 100);
        let response = sign_response(
            &config, &sk, &envelope, response_payload, 200,
        ).unwrap();

        let mut tampered = envelope.clone();
        let idx = flip_idx % tampered.ip_packet.payload.len();
        tampered.ip_packet.payload[idx] ^= 1;
        let err = verify_response(&config, &tampered, &response, 200);
        prop_assert_eq!(err, Err(GatewayError::DigestMismatch));
    }

    /// A response signed by a key NOT matching `config.gateway_pubkey`
    /// fails verification with `InvalidSignature`.
    #[test]
    fn foreign_signer_rejected(
        gateway_seed in any::<u64>(),
        request_payload in prop::collection::vec(any::<u8>(), 0..=256),
        response_payload in prop::collection::vec(any::<u8>(), 0..=256),
    ) {
        let (config, _legit_sk) = build_config(gateway_seed);
        let (_foreign_pk, foreign_sk) = mldsa::keypair();
        let envelope = build_envelope(&config, request_payload, 100);
        let response = sign_response(
            &config, &foreign_sk, &envelope, response_payload, 200,
        ).unwrap();
        let err = verify_response(&config, &envelope, &response, 200);
        prop_assert_eq!(err, Err(GatewayError::InvalidSignature));
    }
}
