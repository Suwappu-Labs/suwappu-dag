//! ML-KEM-768 (FIPS 203) key encapsulation.
//!
//! Used in the GSX DAG L1 on the LTP sealed-session-key surface (paper §10.2),
//! contributing the ≈1,568-byte component of the constant-size on-chain
//! commitment.
//!
//! Wraps the `pqcrypto-mlkem` crate. Construction-shape is fixed at parameter
//! set **ML-KEM-768** (NIST claim 3).

use pqcrypto_mlkem::mlkem768;
use pqcrypto_traits::kem::{Ciphertext as _, PublicKey as _, SecretKey as _, SharedSecret as _};
use subtle::ConstantTimeEq;

use crate::error::CryptoError;

/// ML-KEM-768 public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey(Vec<u8>);

/// ML-KEM-768 secret key.
///
/// `PartialEq` uses constant-time comparison via `subtle::ConstantTimeEq`
/// to prevent timing side-channels (C6 hardening).
#[derive(Debug, Clone)]
pub struct SecretKey(Vec<u8>);

impl PartialEq for SecretKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}
impl Eq for SecretKey {}

/// ML-KEM-768 ciphertext (the sealed session key on the wire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ciphertext(Vec<u8>);

/// ML-KEM-768 derived shared secret.
///
/// `PartialEq` uses constant-time comparison to prevent timing
/// side-channels on shared secret values (C6 hardening).
#[derive(Debug, Clone)]
pub struct SharedSecret(Vec<u8>);

impl PartialEq for SharedSecret {
    fn eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}
impl Eq for SharedSecret {}

impl PublicKey {
    /// Borrow the public-key bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Construct from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        mlkem768::PublicKey::from_bytes(bytes)
            .map(|_| Self(bytes.to_vec()))
            .map_err(|_| CryptoError::MalformedKey("ml-kem-768 public key"))
    }
}

impl SecretKey {
    /// Borrow the secret-key bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Construct from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        mlkem768::SecretKey::from_bytes(bytes)
            .map(|_| Self(bytes.to_vec()))
            .map_err(|_| CryptoError::MalformedKey("ml-kem-768 secret key"))
    }
}

impl Ciphertext {
    /// Borrow the ciphertext bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl SharedSecret {
    /// Borrow the shared-secret bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Generate a fresh ML-KEM-768 keypair from system randomness.
pub fn keypair() -> (PublicKey, SecretKey) {
    let (pk, sk) = mlkem768::keypair();
    (
        PublicKey(pk.as_bytes().to_vec()),
        SecretKey(sk.as_bytes().to_vec()),
    )
}

/// Encapsulate to `pk`, producing `(ciphertext, shared_secret)`.
pub fn encapsulate(pk: &PublicKey) -> Result<(Ciphertext, SharedSecret), CryptoError> {
    let pk_typed = mlkem768::PublicKey::from_bytes(&pk.0)
        .map_err(|_| CryptoError::MalformedKey("ml-kem-768 public key"))?;
    let (ss, ct) = mlkem768::encapsulate(&pk_typed);
    Ok((
        Ciphertext(ct.as_bytes().to_vec()),
        SharedSecret(ss.as_bytes().to_vec()),
    ))
}

/// Decapsulate `ct` against `sk`, recovering the shared secret.
pub fn decapsulate(ct: &Ciphertext, sk: &SecretKey) -> Result<SharedSecret, CryptoError> {
    let sk_typed = mlkem768::SecretKey::from_bytes(&sk.0)
        .map_err(|_| CryptoError::MalformedKey("ml-kem-768 secret key"))?;
    let ct_typed =
        mlkem768::Ciphertext::from_bytes(&ct.0).map_err(|_| CryptoError::DecapsulationFailed)?;
    let ss = mlkem768::decapsulate(&ct_typed, &sk_typed);
    Ok(SharedSecret(ss.as_bytes().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_basic() {
        let (pk, sk) = keypair();
        let (ct, ss_send) = encapsulate(&pk).unwrap();
        let ss_recv = decapsulate(&ct, &sk).unwrap();
        assert_eq!(ss_send, ss_recv);
    }

    #[test]
    fn wrong_secret_key_does_not_recover() {
        let (pk, _sk) = keypair();
        let (_, _sk2) = keypair();
        let (ct, ss_send) = encapsulate(&pk).unwrap();
        let (_, sk_wrong) = keypair();
        // ML-KEM is IND-CCA2: decapsulation with a wrong key returns a
        // deterministic "implicit-rejection" secret that does not equal ss_send.
        let ss_wrong = decapsulate(&ct, &sk_wrong).unwrap();
        assert_ne!(ss_send, ss_wrong);
    }
}
