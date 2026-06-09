//! BLS12-381 aggregate signatures.
//!
//! Used in the GSX DAG L1 on the LTP aggregate-signature surface (paper §10.2),
//! contributing the ≈96-byte component of the constant-size on-chain commitment.
//!
//! Wraps `blst`. Construction is the BLS minimum-pubkey-size variant
//! ("BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_NUL_") which Ethereum and most
//! production BLS deployments use.
//!
//! Migration: paper §12 records BLS12-381 as a classical-cryptography
//! exception zone with target migration to hash-based + SP1-STARK aggregation
//! in 2027–2029.

use blst::{
    min_pk::{AggregatePublicKey, AggregateSignature, PublicKey, SecretKey, Signature},
    BLST_ERROR,
};
use rand::{rngs::OsRng, RngCore};

use crate::error::CryptoError;

/// IETF BLS signature ciphersuite — minimum-pubkey-size, BLS12-381 G2 sigs,
/// SHA-256-based hash-to-curve, random-oracle mode, no AUG.
pub const BLS_DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_NUL_";

/// Generate a fresh BLS keypair from system randomness.
pub fn keypair() -> (PublicKey, SecretKey) {
    let mut ikm = [0u8; 32];
    OsRng.fill_bytes(&mut ikm);
    let sk = SecretKey::key_gen(&ikm, &[]).expect("key_gen with 32-byte IKM is infallible");
    let pk = sk.sk_to_pk();
    (pk, sk)
}

/// Generate a fresh BLS keypair and return the raw serialised bytes.
///
/// Returns `(pk_bytes, sk_bytes)`:
/// - `pk_bytes` is the 48-byte compressed G1 public key.
/// - `sk_bytes` is the 32-byte big-endian secret scalar.
///
/// Convenience wrapper for key-generation tools that need `Vec<u8>` directly
/// rather than the blst concrete types.
pub fn keypair_bytes() -> (Vec<u8>, Vec<u8>) {
    let (pk, sk) = keypair();
    (pk.to_bytes().to_vec(), sk.to_bytes().to_vec())
}

/// Sign a message under a single BLS secret key.
pub fn sign(message: &[u8], sk: &SecretKey) -> Signature {
    sk.sign(message, BLS_DST, &[])
}

/// Verify a single signature against `(pk, message)`.
pub fn verify(message: &[u8], sig: &Signature, pk: &PublicKey) -> Result<(), CryptoError> {
    match sig.verify(true, message, BLS_DST, &[], pk, true) {
        BLST_ERROR::BLST_SUCCESS => Ok(()),
        _ => Err(CryptoError::InvalidSignature),
    }
}

/// Aggregate a set of signatures over the **same** message.
///
/// Returns the aggregate plus the list of public keys that signed. The
/// aggregate verifies against the message and the aggregate public key.
pub fn aggregate(sigs: &[&Signature]) -> Result<Signature, CryptoError> {
    if sigs.is_empty() {
        return Err(CryptoError::BlsAggregation("empty signature set"));
    }
    let agg = AggregateSignature::aggregate(sigs, true)
        .map_err(|_| CryptoError::BlsAggregation("aggregation failed"))?;
    Ok(agg.to_signature())
}

/// Aggregate a set of public keys into a single aggregate public key.
pub fn aggregate_pubkeys(pks: &[&PublicKey]) -> Result<PublicKey, CryptoError> {
    if pks.is_empty() {
        return Err(CryptoError::BlsAggregation("empty pubkey set"));
    }
    let agg = AggregatePublicKey::aggregate(pks, true)
        .map_err(|_| CryptoError::BlsAggregation("pubkey aggregation failed"))?;
    Ok(agg.to_public_key())
}

/// Verify an aggregate signature over a single common `message`, given the
/// aggregate public key.
pub fn verify_aggregate(
    message: &[u8],
    agg_sig: &Signature,
    agg_pk: &PublicKey,
) -> Result<(), CryptoError> {
    verify(message, agg_sig, agg_pk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_single() {
        let (pk, sk) = keypair();
        let msg = b"ltp aggregate-signature surface";
        let sig = sign(msg, &sk);
        verify(msg, &sig, &pk).unwrap();
    }

    #[test]
    fn flipped_message_fails() {
        let (pk, sk) = keypair();
        let sig = sign(b"original", &sk);
        assert!(verify(b"tampered", &sig, &pk).is_err());
    }

    #[test]
    fn aggregate_seven_of_nine() {
        // Paper §10: 7-of-9 super-node attestation quorum.
        let mut pks = Vec::new();
        let mut sks = Vec::new();
        for _ in 0..7 {
            let (pk, sk) = keypair();
            pks.push(pk);
            sks.push(sk);
        }
        let msg = b"corridor attestation height=42";
        let sigs: Vec<Signature> = sks.iter().map(|sk| sign(msg, sk)).collect();
        let sig_refs: Vec<&Signature> = sigs.iter().collect();
        let pk_refs: Vec<&PublicKey> = pks.iter().collect();

        let agg_sig = aggregate(&sig_refs).unwrap();
        let agg_pk = aggregate_pubkeys(&pk_refs).unwrap();
        verify_aggregate(msg, &agg_sig, &agg_pk).unwrap();
    }
}
