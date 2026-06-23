//! SCION-IP-Gateway fallback (paper §6.3).
//!
//! "Inter-validator transport runs on SCION with a SCION-IP-Gateway
//! fallback for external clients."
//!
//! External clients that do not speak SCION natively reach the
//! validator mesh through an SCION-IP-Gateway. The gateway:
//!
//! 1. Receives the client's IP request.
//! 2. Encapsulates it under a SCION path within the validator ISD,
//!    yielding a `GatewayEnvelope`.
//! 3. Forwards the envelope to the destination validator over the
//!    path-authenticated SCION network of DAG-S18.
//! 4. On response, signs `(envelope_digest, response_payload)` under
//!    the gateway's ML-DSA-65 key and returns the `GatewayResponse`
//!    to the client.
//!
//! The security property the fallback must preserve: an attacker on
//! the flat IP infrastructure between the client and the gateway can
//! **drop** or **delay** packets but cannot **forge** responses,
//! because every response is bound to the precise envelope digest and
//! signed by the gateway. Tampering with any byte of the envelope or
//! response invalidates the signature.

use serde::{Deserialize, Serialize};
use suwappu_crypto::{hash, mldsa};
use thiserror::Error;

use crate::scion::{verify_path, AsId, IsdId, Path, ScionError, TrustRootConfig};

/// Per-gateway configuration. Pinned at validator-set time and rotated
/// under the same governance as the SCION TRC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayConfig {
    /// Gateway's ISD.
    pub gateway_isd: IsdId,
    /// Gateway's AS.
    pub gateway_as: AsId,
    /// Gateway's ML-DSA-65 public key (raw bytes).
    pub gateway_pubkey: Vec<u8>,
    /// Trust Root Configuration the gateway operates under.
    pub trc: TrustRootConfig,
}

/// IPv4 / IPv6 packet as observed at the gateway's IP side. Phase-1
/// represents the source/destination as 16-byte arrays (IPv4 is
/// left-padded into this width); the payload is opaque bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpPacket {
    /// Source IP (left-padded to 16 bytes for IPv4).
    pub source_ip: [u8; 16],
    /// Destination IP (left-padded to 16 bytes for IPv4).
    pub dest_ip: [u8; 16],
    /// Opaque payload.
    pub payload: Vec<u8>,
}

/// Envelope encapsulating an `IpPacket` under a SCION-authenticated
/// path within the validator ISD.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayEnvelope {
    /// The IP packet being tunnelled.
    pub ip_packet: IpPacket,
    /// SCION path from the gateway to the destination validator. Must
    /// verify under the gateway's TRC.
    pub scion_path: Path,
    /// Round at which the gateway constructed this envelope.
    pub created_at: u64,
}

impl GatewayEnvelope {
    /// Canonical SHA3-256 digest of this envelope, used as the binding
    /// input for the gateway response signature.
    pub fn canonical_digest(&self) -> [u8; 32] {
        let mut blob = Vec::with_capacity(
            16 + 16 + self.ip_packet.payload.len() + 8 + self.scion_path.hops.len() * 16,
        );
        blob.extend_from_slice(&self.ip_packet.source_ip);
        blob.extend_from_slice(&self.ip_packet.dest_ip);
        blob.extend_from_slice(&(self.ip_packet.payload.len() as u32).to_be_bytes());
        blob.extend_from_slice(&self.ip_packet.payload);
        blob.extend_from_slice(&self.scion_path.isd.to_be_bytes());
        blob.extend_from_slice(&self.scion_path.created_at.to_be_bytes());
        blob.extend_from_slice(&(self.scion_path.hops.len() as u32).to_be_bytes());
        for hop in &self.scion_path.hops {
            blob.extend_from_slice(&hop.mac);
        }
        blob.extend_from_slice(&self.created_at.to_be_bytes());
        hash::sha3_256_domain(b"SUWAPPU-GATEWAY-ENV-V1", &blob)
    }
}

/// Gateway response to a client's IP request. Binds the envelope
/// digest and the response payload under the gateway's ML-DSA-65
/// signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayResponse {
    /// SHA3-256 digest of the envelope being responded to.
    pub request_digest: [u8; 32],
    /// Opaque response payload.
    pub response_payload: Vec<u8>,
    /// ML-DSA-65 signature over `(SUWAPPU-GATEWAY-RESP-V1, request_digest,
    /// response_payload_len, response_payload, created_at)`.
    pub gateway_signature: Vec<u8>,
    /// Round at which the gateway signed this response.
    pub created_at: u64,
}

/// Signing payload — what `gateway_signature` covers.
fn response_signing_digest(
    request_digest: &[u8; 32],
    response_payload: &[u8],
    created_at: u64,
) -> [u8; 32] {
    let mut blob = Vec::with_capacity(32 + 4 + response_payload.len() + 8);
    blob.extend_from_slice(request_digest);
    blob.extend_from_slice(&(response_payload.len() as u32).to_be_bytes());
    blob.extend_from_slice(response_payload);
    blob.extend_from_slice(&created_at.to_be_bytes());
    hash::sha3_256_domain(b"SUWAPPU-GATEWAY-RESP-V1", &blob)
}

/// Errors emitted by the gateway pipeline.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum GatewayError {
    /// The embedded SCION path did not verify under the gateway's TRC.
    #[error("scion path verification failed: {0:?}")]
    PathVerificationFailed(ScionError),

    /// The gateway's public key bytes did not parse as ML-DSA-65.
    #[error("malformed gateway public key")]
    MalformedGatewayKey,

    /// The response's `request_digest` does not match the envelope's
    /// canonical digest. The client should not trust this response.
    #[error("response request_digest does not match envelope digest")]
    DigestMismatch,

    /// The response signature did not verify under the gateway's key.
    #[error("response signature verification failed")]
    InvalidSignature,

    /// Response signature bytes did not parse as ML-DSA-65.
    #[error("malformed response signature")]
    MalformedSignature,
}

/// The gateway produces a signed response to a client's request.
///
/// Validates the embedded SCION path before signing. The gateway must
/// hold the matching ML-DSA-65 private key for
/// `config.gateway_pubkey`.
pub fn sign_response(
    config: &GatewayConfig,
    sk: &mldsa::SecretKey,
    envelope: &GatewayEnvelope,
    response_payload: Vec<u8>,
    now: u64,
) -> Result<GatewayResponse, GatewayError> {
    verify_path(&envelope.scion_path, &config.trc, now)
        .map_err(GatewayError::PathVerificationFailed)?;

    let request_digest = envelope.canonical_digest();
    let signing_digest = response_signing_digest(&request_digest, &response_payload, now);
    let sig = mldsa::sign(&signing_digest, sk).map_err(|_| GatewayError::InvalidSignature)?;
    Ok(GatewayResponse {
        request_digest,
        response_payload,
        gateway_signature: sig.as_bytes().to_vec(),
        created_at: now,
    })
}

/// Verify a gateway response against the envelope.
///
/// Returns `Ok(())` iff:
///
/// 1. The embedded SCION path verifies (BGP-class attack mitigation).
/// 2. `response.request_digest == envelope.canonical_digest()`.
/// 3. The gateway signature verifies under `config.gateway_pubkey`
///    over the canonical signing digest.
pub fn verify_response(
    config: &GatewayConfig,
    envelope: &GatewayEnvelope,
    response: &GatewayResponse,
    now: u64,
) -> Result<(), GatewayError> {
    verify_path(&envelope.scion_path, &config.trc, now)
        .map_err(GatewayError::PathVerificationFailed)?;

    let envelope_digest = envelope.canonical_digest();
    if envelope_digest != response.request_digest {
        return Err(GatewayError::DigestMismatch);
    }

    let pk = mldsa::PublicKey::from_bytes(&config.gateway_pubkey)
        .map_err(|_| GatewayError::MalformedGatewayKey)?;
    let sig = mldsa::Signature::from_bytes(&response.gateway_signature)
        .map_err(|_| GatewayError::MalformedSignature)?;
    let signing_digest = response_signing_digest(
        &response.request_digest,
        &response.response_payload,
        response.created_at,
    );
    mldsa::verify(&signing_digest, &sig, &pk).map_err(|_| GatewayError::InvalidSignature)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::scion::{seal_path, HopField};

    fn build_config_and_path() -> (GatewayConfig, Path, mldsa::SecretKey) {
        // TRC authorizes AS 10 with a deterministic key.
        let mut as_keys = BTreeMap::new();
        as_keys.insert(10u32, [0xABu8; 32]);
        let trc = TrustRootConfig {
            isd: 1,
            version: 1,
            as_keys,
            valid_until: 1_000_000,
        };

        // Honest path with a single hop.
        let hops = vec![HopField {
            isd_as: (1, 10),
            ingress_iface: 1,
            egress_iface: 2,
            expiration_round: 1_000,
            mac: [0u8; 16],
        }];
        let path = seal_path(1, 100, hops, &trc).unwrap();

        // Gateway key.
        let (pk, sk) = mldsa::keypair();
        let config = GatewayConfig {
            gateway_isd: 1,
            gateway_as: 10,
            gateway_pubkey: pk.as_bytes().to_vec(),
            trc,
        };
        (config, path, sk)
    }

    fn build_envelope(path: Path, payload: Vec<u8>) -> GatewayEnvelope {
        GatewayEnvelope {
            ip_packet: IpPacket {
                source_ip: [10; 16],
                dest_ip: [20; 16],
                payload,
            },
            scion_path: path,
            created_at: 100,
        }
    }

    #[test]
    fn round_trip_works() {
        let (config, path, sk) = build_config_and_path();
        let envelope = build_envelope(path, b"hello".to_vec());
        let response = sign_response(&config, &sk, &envelope, b"hi".to_vec(), 200).unwrap();
        verify_response(&config, &envelope, &response, 200).unwrap();
    }

    #[test]
    fn tampered_response_rejected() {
        let (config, path, sk) = build_config_and_path();
        let envelope = build_envelope(path, b"hello".to_vec());
        let mut response = sign_response(&config, &sk, &envelope, b"hi".to_vec(), 200).unwrap();
        response.response_payload[0] ^= 1;
        let err = verify_response(&config, &envelope, &response, 200);
        assert_eq!(err, Err(GatewayError::InvalidSignature));
    }

    #[test]
    fn path_unauthentic_rejected() {
        let (config, mut path, sk) = build_config_and_path();
        path.hops[0].mac[0] ^= 1; // tamper the path
        let envelope = build_envelope(path, b"hello".to_vec());
        let err = sign_response(&config, &sk, &envelope, b"hi".to_vec(), 200);
        assert!(matches!(err, Err(GatewayError::PathVerificationFailed(_))));
    }

    #[test]
    fn foreign_signer_rejected() {
        let (config, path, _sk) = build_config_and_path();
        let (_foreign_pk, foreign_sk) = mldsa::keypair();
        let envelope = build_envelope(path, b"hello".to_vec());
        let response = sign_response(&config, &foreign_sk, &envelope, b"hi".to_vec(), 200).unwrap();
        let err = verify_response(&config, &envelope, &response, 200);
        assert_eq!(err, Err(GatewayError::InvalidSignature));
    }

    #[test]
    fn envelope_tamper_rejected() {
        let (config, path, sk) = build_config_and_path();
        let envelope = build_envelope(path, b"hello".to_vec());
        let response = sign_response(&config, &sk, &envelope, b"hi".to_vec(), 200).unwrap();
        // Client tampers the envelope before verification.
        let mut tampered = envelope.clone();
        tampered.ip_packet.payload[0] ^= 1;
        let err = verify_response(&config, &tampered, &response, 200);
        assert_eq!(err, Err(GatewayError::DigestMismatch));
    }
}
