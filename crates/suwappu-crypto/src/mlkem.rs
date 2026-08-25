//! ML-KEM-768 (FIPS 203) key encapsulation.
//!
//! Used in the SUWAPPU DAG L1 on the LTP sealed-session-key surface (paper §10.2),
//! contributing the ≈1,568-byte component of the constant-size on-chain
//! commitment.
//!
//! Wraps the pure-Rust `ml-kem` crate (RustCrypto). Construction-shape is fixed
//! at parameter set **ML-KEM-768** (NIST claim 3).
//!
//! Replaced the `pqcrypto-mlkem` PQClean bindings, unmaintained since PQClean was
//! archived in 2026 (RUSTSEC-2026-0161). Wire encodings are unchanged: the FIPS
//! 203 byte strings, cross-checked in `tests/pqclean_interop.rs`.
//!
//! The secret key handled here is the **expanded** 2400-byte FIPS 203
//! decapsulation key, matching PQClean and existing keystores — not the 64-byte
//! seed form.

// The expanded (2400-byte) decapsulation-key encoding is deprecated upstream in
// favour of the 64-byte seed form. It is still what PQClean emitted and what
// existing keystores and on-disk validator keys contain, so it is deliberately
// retained here; switching to seeds would invalidate every stored key.
#[allow(deprecated)]
use ml_kem::ExpandedKeyEncoding as _;
use ml_kem::array::Array;
use ml_kem::kem::{Decapsulate as _, Encapsulate as _, Kem as _, KeyExport as _};
use ml_kem::{DecapsulationKey, EncapsulationKey, MlKem768};

type Ek = EncapsulationKey<MlKem768>;
type Dk = DecapsulationKey<MlKem768>;

fn encapsulation_key(bytes: &[u8]) -> Result<Ek, CryptoError> {
    let arr =
        Array::try_from(bytes).map_err(|_| CryptoError::MalformedKey("ml-kem-768 public key"))?;
    Ek::new(&arr).map_err(|_| CryptoError::MalformedKey("ml-kem-768 public key"))
}

#[allow(deprecated)]
fn decapsulation_key(bytes: &[u8]) -> Result<Dk, CryptoError> {
    let arr =
        Array::try_from(bytes).map_err(|_| CryptoError::MalformedKey("ml-kem-768 secret key"))?;
    Dk::from_expanded_bytes(&arr).map_err(|_| CryptoError::MalformedKey("ml-kem-768 secret key"))
}

use crate::error::CryptoError;

/// Byte length of an ML-KEM-768 encapsulation (public) key.
pub const PUBLIC_KEY_LEN: usize = 1184;
/// Byte length of an expanded ML-KEM-768 decapsulation (secret) key.
pub const SECRET_KEY_LEN: usize = 2400;
/// Byte length of an ML-KEM-768 ciphertext.
pub const CIPHERTEXT_LEN: usize = 1088;

/// ML-KEM-768 public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey(Vec<u8>);

/// ML-KEM-768 secret key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretKey(Vec<u8>);

/// ML-KEM-768 ciphertext (the sealed session key on the wire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ciphertext(Vec<u8>);

/// ML-KEM-768 derived shared secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSecret(Vec<u8>);

impl PublicKey {
    /// Borrow the public-key bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Construct from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        encapsulation_key(bytes).map(|_| Self(bytes.to_vec()))
    }
}

impl SecretKey {
    /// Borrow the secret-key bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Construct from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        decapsulation_key(bytes).map(|_| Self(bytes.to_vec()))
    }
}

impl Ciphertext {
    /// Borrow the ciphertext bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Construct from wire bytes, checking the FIPS 203 length.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != CIPHERTEXT_LEN {
            return Err(CryptoError::DecapsulationFailed);
        }
        Ok(Self(bytes.to_vec()))
    }
}

impl SharedSecret {
    /// Borrow the shared-secret bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Generate a fresh ML-KEM-768 keypair from system randomness.
#[allow(deprecated)]
pub fn keypair() -> (PublicKey, SecretKey) {
    let (dk, ek) = MlKem768::generate_keypair();
    (
        PublicKey(ek.to_bytes().to_vec()),
        SecretKey(dk.to_expanded_bytes().to_vec()),
    )
}

/// Encapsulate to `pk`, producing `(ciphertext, shared_secret)`.
pub fn encapsulate(pk: &PublicKey) -> Result<(Ciphertext, SharedSecret), CryptoError> {
    let ek = encapsulation_key(&pk.0)?;
    let (ct, ss) = ek.encapsulate();
    Ok((
        Ciphertext(ct.to_vec()),
        SharedSecret(ss.to_vec()),
    ))
}

/// Decapsulate `ct` against `sk`, recovering the shared secret.
pub fn decapsulate(ct: &Ciphertext, sk: &SecretKey) -> Result<SharedSecret, CryptoError> {
    let dk = decapsulation_key(&sk.0)?;
    let ct_typed =
        Array::try_from(&ct.0[..]).map_err(|_| CryptoError::DecapsulationFailed)?;
    let ss = dk.decapsulate(&ct_typed);
    Ok(SharedSecret(ss.to_vec()))
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
