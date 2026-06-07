//! On-chain ML-DSA-65 (FIPS 204) signature-verification precompile core.
//!
//! This crate implements the load-bearing, security-critical part of Suwappu's
//! on-chain post-quantum signature verification (audit program P5b, Phase 1):
//! given `pubkey || signature || message`, it returns a 32-byte EVM word that
//! is `1` iff the ML-DSA-65 detached signature is valid, else `0`.
//!
//! It wraps the same NIST PQC reference verifier (`pqcrypto-mldsa`, ML-DSA-65)
//! used by `suwappu-crypto` for validator/consensus signatures, so it is genuinely
//! FIPS-204 post-quantum sound — no SNARK wrapper, no scheme substitution
//! (contrast the SP1→Groth16/BN254 path, which is Shor-broken; see
//! `docs/security/audits/suwappu/P5b_ONCHAIN_PQ.md` in suwappu-lattice-protocol).
//!
//! Two consumers:
//!   1. **Suwappu DAG EVM precompile** (via `suwappu-revm`): register at a fixed address
//!      so bridge contracts can `staticcall` it during mint/unlock/finalize.
//!   2. **Suwappu DAG intent handler** (ship-now path while the EVM substrate is
//!      finished): call [`verify`] directly from the execution substrate.
//!
//! The verifier is pure and stateless: output is a deterministic function of
//! the input bytes, never panics, and rejects all malformed inputs as `0`.

#![forbid(unsafe_code)]

use pqcrypto_mldsa::mldsa65;
// `from_bytes` is a trait associated function called via path
// (`mldsa65::PublicKey::from_bytes`); the traits must be in scope for
// resolution, but rustc's unused-import lint does not count path-form
// associated-fn calls as usage (known false positive). Removing these
// breaks compilation, so suppress the spurious warning.
#[allow(unused_imports)]
use pqcrypto_traits::sign::{DetachedSignature, PublicKey};

/// ML-DSA-65 (FIPS 204) public-key length in bytes.
pub const PK_LEN: usize = 1952;
/// ML-DSA-65 (FIPS 204) detached-signature length in bytes.
pub const SIG_LEN: usize = 3309;

/// Minimum valid precompile input length (`pubkey || signature`, empty message).
pub const MIN_INPUT_LEN: usize = PK_LEN + SIG_LEN;

/// EVM word for a verified signature (`0x00..01`).
pub const WORD_TRUE: [u8; 32] = {
    let mut w = [0u8; 32];
    w[31] = 1;
    w
};
/// EVM word for a rejected signature (`0x00..00`).
pub const WORD_FALSE: [u8; 32] = [0u8; 32];

/// Verify an ML-DSA-65 detached signature from a flat precompile input.
///
/// Input ABI (tight, no padding):
/// ```text
///   pubkey   : bytes[0          .. 1952]            (PK_LEN)
///   signature: bytes[1952       .. 1952+3309]       (SIG_LEN)
///   message  : bytes[1952+3309  .. ]                (variable, may be empty)
/// ```
/// Returns [`WORD_TRUE`] iff the signature is valid for `message` under
/// `pubkey`, else [`WORD_FALSE`]. Never panics; any malformed input
/// (short, bad key/sig encoding, invalid signature) maps to [`WORD_FALSE`].
#[must_use]
pub fn verify(input: &[u8]) -> [u8; 32] {
    if input.len() < MIN_INPUT_LEN {
        return WORD_FALSE;
    }
    let pk_bytes = &input[..PK_LEN];
    let sig_bytes = &input[PK_LEN..MIN_INPUT_LEN];
    let message = &input[MIN_INPUT_LEN..];

    let pk = match mldsa65::PublicKey::from_bytes(pk_bytes) {
        Ok(p) => p,
        Err(_) => return WORD_FALSE,
    };
    let sig = match mldsa65::DetachedSignature::from_bytes(sig_bytes) {
        Ok(s) => s,
        Err(_) => return WORD_FALSE,
    };
    match mldsa65::verify_detached_signature(&sig, message, &pk) {
        Ok(()) => WORD_TRUE,
        // VerificationError is #[non_exhaustive]; any error rejects.
        Err(_) => WORD_FALSE,
    }
}

/// Convenience: `true` iff [`verify`] returns [`WORD_TRUE`].
#[must_use]
pub fn is_valid(input: &[u8]) -> bool {
    verify(input) == WORD_TRUE
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use pqcrypto_traits::sign::{DetachedSignature, PublicKey};

    fn build(pk: &[u8], sig: &[u8], msg: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(pk.len() + sig.len() + msg.len());
        v.extend_from_slice(pk);
        v.extend_from_slice(sig);
        v.extend_from_slice(msg);
        v
    }

    const MSG: &[u8] = b"suwappu-commit:chainid=8453|commitId=0xabcd|amount=1000|recipient=0xBEEF";

    #[test]
    fn fips204_sizes_match_constants() {
        let (pk, sk) = mldsa65::keypair();
        assert_eq!(pk.as_bytes().len(), PK_LEN);
        let sig = mldsa65::detached_sign(MSG, &sk);
        assert_eq!(sig.as_bytes().len(), SIG_LEN);
    }

    #[test]
    fn valid_signature_accepted() {
        let (pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        let input = build(pk.as_bytes(), sig.as_bytes(), MSG);
        assert_eq!(verify(&input), WORD_TRUE);
        assert!(is_valid(&input));
    }

    #[test]
    fn tampered_message_rejected() {
        let (pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        let mut m = MSG.to_vec();
        m[0] ^= 0xff;
        assert_eq!(
            verify(&build(pk.as_bytes(), sig.as_bytes(), &m)),
            WORD_FALSE
        );
    }

    #[test]
    fn tampered_signature_rejected() {
        let (pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        let mut s = sig.as_bytes().to_vec();
        s[100] ^= 0x01;
        assert_eq!(verify(&build(pk.as_bytes(), &s, MSG)), WORD_FALSE);
    }

    #[test]
    fn wrong_key_rejected() {
        let (_pk, sk) = mldsa65::keypair();
        let (pk2, _sk2) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        assert_eq!(
            verify(&build(pk2.as_bytes(), sig.as_bytes(), MSG)),
            WORD_FALSE
        );
    }

    #[test]
    fn truncated_input_rejected_no_panic() {
        let (pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        let full = build(pk.as_bytes(), sig.as_bytes(), MSG);
        assert_eq!(verify(&full[..MIN_INPUT_LEN - 1]), WORD_FALSE);
        assert_eq!(verify(&[]), WORD_FALSE);
        assert_eq!(verify(&[0u8; 10]), WORD_FALSE);
    }

    #[test]
    fn empty_message_signature_accepted() {
        let (pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(b"", &sk);
        assert_eq!(
            verify(&build(pk.as_bytes(), sig.as_bytes(), b"")),
            WORD_TRUE
        );
    }

    #[test]
    fn malformed_key_bytes_rejected() {
        // right length, garbage content -> from_bytes may accept but verify fails,
        // or from_bytes rejects; either way WORD_FALSE, no panic.
        let (_pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        let junk_pk = vec![0xABu8; PK_LEN];
        assert_eq!(verify(&build(&junk_pk, sig.as_bytes(), MSG)), WORD_FALSE);
    }
}
