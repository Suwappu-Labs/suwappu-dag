//! ML-DSA-65 (FIPS 204) digital signatures.
//!
//! Used in the GSX DAG L1 on the Authority-Ring signing surface and on the LTP
//! integrity surface (paper §3.3, §10).
//!
//! Wraps the `pqcrypto-mldsa` crate, which is the NIST PQC reference port.
//! Construction-shape is fixed at parameter set **ML-DSA-65** (NIST claim 3).

use pqcrypto_mldsa::mldsa65;
use pqcrypto_traits::sign::{
    DetachedSignature as _, PublicKey as _, SecretKey as _, SignedMessage as _,
};

use crate::error::CryptoError;

/// ML-DSA-65 public key, byte-serialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey(Vec<u8>);

/// ML-DSA-65 secret key, byte-serialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretKey(Vec<u8>);

/// ML-DSA-65 detached signature, byte-serialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature(Vec<u8>);

impl PublicKey {
    /// Borrow the public-key bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Construct from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        mldsa65::PublicKey::from_bytes(bytes)
            .map(|_| Self(bytes.to_vec()))
            .map_err(|_| CryptoError::MalformedKey("ml-dsa-65 public key"))
    }
}

impl SecretKey {
    /// Borrow the secret-key bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Construct from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        mldsa65::SecretKey::from_bytes(bytes)
            .map(|_| Self(bytes.to_vec()))
            .map_err(|_| CryptoError::MalformedKey("ml-dsa-65 secret key"))
    }
}

impl Signature {
    /// Borrow the signature bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Construct from wire bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        mldsa65::DetachedSignature::from_bytes(bytes)
            .map(|_| Self(bytes.to_vec()))
            .map_err(|_| CryptoError::InvalidSignature)
    }
}

/// Generate a fresh ML-DSA-65 keypair from system randomness.
pub fn keypair() -> (PublicKey, SecretKey) {
    let (pk, sk) = mldsa65::keypair();
    (
        PublicKey(pk.as_bytes().to_vec()),
        SecretKey(sk.as_bytes().to_vec()),
    )
}

/// Produce a detached signature over `message` using `sk`.
pub fn sign(message: &[u8], sk: &SecretKey) -> Result<Signature, CryptoError> {
    let sk_typed = mldsa65::SecretKey::from_bytes(&sk.0)
        .map_err(|_| CryptoError::MalformedKey("ml-dsa-65 secret key"))?;
    let sig = mldsa65::detached_sign(message, &sk_typed);
    Ok(Signature(sig.as_bytes().to_vec()))
}

/// Verify a detached signature on `message` against `pk`.
pub fn verify(message: &[u8], sig: &Signature, pk: &PublicKey) -> Result<(), CryptoError> {
    let pk_typed = mldsa65::PublicKey::from_bytes(&pk.0)
        .map_err(|_| CryptoError::MalformedKey("ml-dsa-65 public key"))?;
    let sig_typed = mldsa65::DetachedSignature::from_bytes(&sig.0)
        .map_err(|_| CryptoError::InvalidSignature)?;
    mldsa65::verify_detached_signature(&sig_typed, message, &pk_typed)
        .map_err(|_| CryptoError::InvalidSignature)
}

/// Convenience: signed-message form (signature || message) for paper §10.2
/// constant-size commitment composition.
pub fn sign_attached(message: &[u8], sk: &SecretKey) -> Result<Vec<u8>, CryptoError> {
    let sk_typed = mldsa65::SecretKey::from_bytes(&sk.0)
        .map_err(|_| CryptoError::MalformedKey("ml-dsa-65 secret key"))?;
    let signed = mldsa65::sign(message, &sk_typed);
    Ok(signed.as_bytes().to_vec())
}

/// Convenience: verify a signed-message form, returning the recovered message.
pub fn open_attached(signed: &[u8], pk: &PublicKey) -> Result<Vec<u8>, CryptoError> {
    let pk_typed = mldsa65::PublicKey::from_bytes(&pk.0)
        .map_err(|_| CryptoError::MalformedKey("ml-dsa-65 public key"))?;
    let signed_typed =
        mldsa65::SignedMessage::from_bytes(signed).map_err(|_| CryptoError::InvalidSignature)?;
    mldsa65::open(&signed_typed, &pk_typed).map_err(|_| CryptoError::InvalidSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_basic() {
        let (pk, sk) = keypair();
        let msg = b"gsx dag l1 attestation";
        let sig = sign(msg, &sk).unwrap();
        verify(msg, &sig, &pk).unwrap();
    }

    #[test]
    fn flipped_message_fails() {
        let (pk, sk) = keypair();
        let sig = sign(b"original", &sk).unwrap();
        assert!(verify(b"tampered", &sig, &pk).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let (_pk1, sk1) = keypair();
        let (pk2, _sk2) = keypair();
        let sig = sign(b"msg", &sk1).unwrap();
        assert!(verify(b"msg", &sig, &pk2).is_err());
    }

    #[test]
    fn attached_roundtrip() {
        let (pk, sk) = keypair();
        let msg = b"gsx-dag attached";
        let signed = sign_attached(msg, &sk).unwrap();
        let recovered = open_attached(&signed, &pk).unwrap();
        assert_eq!(recovered, msg);
    }
}
