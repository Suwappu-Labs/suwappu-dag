//! ML-DSA-65 (FIPS 204) digital signatures.
//!
//! Used in the SUWAPPU DAG L1 on the Authority-Ring signing surface and on the LTP
//! integrity surface (paper §3.3, §10).
//!
//! Wraps the pure-Rust `ml-dsa` crate (RustCrypto). Construction-shape is fixed
//! at parameter set **ML-DSA-65** (NIST claim 3).
//!
//! Replaced the `pqcrypto-mldsa` PQClean bindings, unmaintained since PQClean was
//! archived in 2026 (RUSTSEC-2026-0162). The on-the-wire encodings are unchanged:
//! keys, secret keys and signatures are the FIPS 204 byte strings, and
//! `tests/pqclean_interop.rs` cross-verifies against the implementation this
//! replaced.
//!
//! Note the secret key handled here is the **expanded** 4032-byte FIPS 204 signing
//! key, matching what PQClean produced and what existing keystores hold — not the
//! 32-byte seed that `ml_dsa::SigningKey` defaults to.

use ml_dsa::{
    EncodedSignature, EncodedVerifyingKey, ExpandedSigningKey, ExpandedSigningKeyBytes, MlDsa65,
    Signature as MlDsaSignature, SigningKey, VerifyingKey,
};

use crate::error::CryptoError;

/// Empty signing context, matching the PQClean bindings this replaced.
const CTX: &[u8] = b"";

/// Byte length of an ML-DSA-65 detached signature (FIPS 204).
pub const SIGNATURE_LEN: usize = 3309;
/// Byte length of an ML-DSA-65 public key (FIPS 204).
pub const PUBLIC_KEY_LEN: usize = 1952;
/// Byte length of an expanded ML-DSA-65 secret key (FIPS 204).
pub const SECRET_KEY_LEN: usize = 4032;

// The expanded (4032-byte) signing-key encoding is deprecated upstream in favour
// of the 32-byte seed. It is what PQClean emitted and what existing validator
// keystores hold, so it is retained deliberately: moving to seeds would
// invalidate every stored secret key.
#[allow(deprecated)]
fn expanded_sk(bytes: &[u8]) -> Result<ExpandedSigningKey<MlDsa65>, CryptoError> {
    let enc = ExpandedSigningKeyBytes::<MlDsa65>::try_from(bytes)
        .map_err(|_| CryptoError::MalformedKey("ml-dsa-65 secret key"))?;
    Ok(ExpandedSigningKey::<MlDsa65>::from_expanded(&enc))
}

fn verifying_key(bytes: &[u8]) -> Result<VerifyingKey<MlDsa65>, CryptoError> {
    let enc = EncodedVerifyingKey::<MlDsa65>::try_from(bytes)
        .map_err(|_| CryptoError::MalformedKey("ml-dsa-65 public key"))?;
    Ok(VerifyingKey::<MlDsa65>::decode(&enc))
}

fn signature(bytes: &[u8]) -> Result<MlDsaSignature<MlDsa65>, CryptoError> {
    let enc =
        EncodedSignature::<MlDsa65>::try_from(bytes).map_err(|_| CryptoError::InvalidSignature)?;
    MlDsaSignature::<MlDsa65>::decode(&enc).ok_or(CryptoError::InvalidSignature)
}

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
        verifying_key(bytes).map(|_| Self(bytes.to_vec()))
    }
}

impl SecretKey {
    /// Borrow the secret-key bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Construct from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        expanded_sk(bytes).map(|_| Self(bytes.to_vec()))
    }
}

impl Signature {
    /// Borrow the signature bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Construct from wire bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        signature(bytes).map(|_| Self(bytes.to_vec()))
    }
}

/// Generate a fresh ML-DSA-65 keypair from system randomness.
#[allow(deprecated)]
pub fn keypair() -> (PublicKey, SecretKey) {
    let mut seed = ml_dsa::Seed::default();
    crate::random_bytes(&mut seed);
    let sk = SigningKey::<MlDsa65>::from_seed(&seed);
    let pk = sk.expanded_key().verifying_key();
    (
        PublicKey(pk.encode().to_vec()),
        SecretKey(sk.expanded_key().to_expanded().to_vec()),
    )
}

/// Produce a detached signature over `message` using `sk`.
pub fn sign(message: &[u8], sk: &SecretKey) -> Result<Signature, CryptoError> {
    let sk_typed = expanded_sk(&sk.0)?;
    // Hedged (randomized) signing with an empty context, matching the PQClean
    // bindings this replaced.
    let sig = sk_typed
        .sign_randomized(message, CTX, &mut crate::rng::OsRng)
        .map_err(|_| CryptoError::InvalidSignature)?;
    Ok(Signature(sig.encode().to_vec()))
}

/// Verify a detached signature on `message` against `pk`.
pub fn verify(message: &[u8], sig: &Signature, pk: &PublicKey) -> Result<(), CryptoError> {
    let pk_typed = verifying_key(&pk.0)?;
    let sig_typed = signature(&sig.0)?;
    if pk_typed.verify_with_context(message, CTX, &sig_typed) {
        Ok(())
    } else {
        Err(CryptoError::InvalidSignature)
    }
}

/// Convenience: signed-message form (signature || message) for paper §10.2
/// constant-size commitment composition.
pub fn sign_attached(message: &[u8], sk: &SecretKey) -> Result<Vec<u8>, CryptoError> {
    // PQClean's signed-message form is `signature || message`; preserved exactly
    // so anything already on-chain keeps parsing. Asserted in tests/pqclean_interop.rs.
    let sig = sign(message, sk)?;
    let mut out = sig.0;
    out.extend_from_slice(message);
    Ok(out)
}

/// Convenience: verify a signed-message form, returning the recovered message.
pub fn open_attached(signed: &[u8], pk: &PublicKey) -> Result<Vec<u8>, CryptoError> {
    let siglen = SIGNATURE_LEN;
    if signed.len() < siglen {
        return Err(CryptoError::InvalidSignature);
    }
    let (sig_bytes, message) = signed.split_at(siglen);
    let sig = Signature(sig_bytes.to_vec());
    verify(message, &sig, pk)?;
    Ok(message.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_basic() {
        let (pk, sk) = keypair();
        let msg = b"suwappu dag l1 attestation";
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
        let msg = b"suwappu-dag attached";
        let signed = sign_attached(msg, &sk).unwrap();
        let recovered = open_attached(&signed, &pk).unwrap();
        assert_eq!(recovered, msg);
    }
}
