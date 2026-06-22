//! Hashing primitives.
//!
//! SHA3-256 (FIPS 202) is the canonical hash on the LTP commitment surface
//! (paper §10.2) and the SCION-routed transport layer. Poseidon2 is provided
//! for arithmetic-friendly contexts (state-tree leaf encodings, future PlonK
//! circuits) but is intentionally **not** used on the LTP integrity surface.
//!
//! HKDF (RFC 5869) over SHA3-256 is exposed for multi-output key derivation
//! (nullifier-key + viewing-key + spend-key derivation from a single root
//! seed in the Track H PQ-confidential transfer construction).

use hkdf::Hkdf;
use sha3::{Digest, Sha3_256};

/// Length of a SHA3-256 digest in bytes.
pub const SHA3_256_BYTES: usize = 32;

/// Domain-separation tag for L2 note commitments
/// `cm = SHA3-256-domain(SUWAPPU_L2_NOTE_COMMIT_V1, v_le ‖ r ‖ pk_owner)`.
/// See Track H confidential-transfer construction.
pub const SUWAPPU_L2_NOTE_COMMIT_V1: &[u8] = b"gsx-l2-note-commit-v1";

/// Domain-separation tag for L2 nullifiers
/// `nf = SHA3-256-domain(SUWAPPU_L2_NULLIFIER_V1, nk ‖ cm ‖ position_le)`.
pub const SUWAPPU_L2_NULLIFIER_V1: &[u8] = b"gsx-l2-nullifier-v1";

/// Domain-separation tag for L2 nullifier-key derivation
/// `nk = SHA3-256-domain(SUWAPPU_L2_NF_KEY_V1, sk_seed)`.
pub const SUWAPPU_L2_NF_KEY_V1: &[u8] = b"gsx-l2-nf-key-v1";

/// Domain-separation tag for L2 viewing-key seed derivation
/// `viewing_seed = SHA3-256-domain(SUWAPPU_L2_VIEWING_KEY_V1, sk_seed)`;
/// `(ek_view, dk_view) = ML-KEM-768.KeyGen(viewing_seed)`.
pub const SUWAPPU_L2_VIEWING_KEY_V1: &[u8] = b"gsx-l2-viewing-key-v1";

/// Domain-separation tag for L2 confidential-account address derivation
/// `addr = SHA3-256-domain(SUWAPPU_L2_ADDRESS_V1, pk_owner_mldsa)[..20]`.
pub const SUWAPPU_L2_ADDRESS_V1: &[u8] = b"gsx-l2-address-v1";

/// HKDF-SHA3-256 multi-output key derivation (RFC 5869).
///
/// Expands `ikm` under `salt` into `out_len` bytes labeled by `info`.
/// Used in Track H confidential-transfer derivations and elsewhere when
/// multiple cryptographically-independent outputs must be derived from a
/// single root seed.
///
/// **Failure mode**: `out_len` must be at most `255 * 32 = 8160` bytes
/// (the HKDF-SHA3-256 expansion limit). Larger requests panic via the
/// underlying `hkdf` crate; callers MUST validate `out_len <= 8160`.
///
/// # Example
///
/// ```
/// use suwappu_crypto::hash::hkdf_sha3_256;
///
/// let seed = b"\xab\xcd\xef\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\
///              \x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d";
/// let nf_key = hkdf_sha3_256(b"gsx-l2-derive", seed, b"nullifier-key", 32);
/// let view_seed = hkdf_sha3_256(b"gsx-l2-derive", seed, b"viewing-key", 32);
/// assert_ne!(nf_key, view_seed);
/// assert_eq!(nf_key.len(), 32);
/// ```
pub fn hkdf_sha3_256(salt: &[u8], ikm: &[u8], info: &[u8], out_len: usize) -> Vec<u8> {
    debug_assert!(
        out_len <= 255 * SHA3_256_BYTES,
        "HKDF-SHA3-256 expansion limit is 255 * 32 = 8160 bytes; got {out_len}"
    );
    let hk = Hkdf::<Sha3_256>::new(Some(salt), ikm);
    let mut out = vec![0u8; out_len];
    hk.expand(info, &mut out)
        .expect("hkdf expand: out_len within RFC 5869 limit");
    out
}

/// Compute the SHA3-256 digest of `data`.
///
/// Constant-time-safe over the digest output; input length is observable.
pub fn sha3_256(data: &[u8]) -> [u8; SHA3_256_BYTES] {
    let mut hasher = Sha3_256::new();
    hasher.update(data);
    let out = hasher.finalize();
    let mut bytes = [0u8; SHA3_256_BYTES];
    bytes.copy_from_slice(&out);
    bytes
}

/// Compute the domain-separated SHA3-256 digest
/// `H(len(tag) as u32 BE || tag || data)`.
///
/// Length-prefixing the tag prevents the boundary-shift attack a single-byte
/// separator is vulnerable to when `data[0]` collides with the separator byte.
/// Tag length is bounded at `u32::MAX` bytes — domain tags above 4 GiB are
/// rejected via `debug_assert`.
pub fn sha3_256_domain(tag: &[u8], data: &[u8]) -> [u8; SHA3_256_BYTES] {
    debug_assert!(
        tag.len() <= u32::MAX as usize,
        "tag length exceeds u32::MAX"
    );
    let tag_len = (tag.len() as u32).to_be_bytes();
    let mut hasher = Sha3_256::new();
    hasher.update(tag_len);
    hasher.update(tag);
    hasher.update(data);
    let out = hasher.finalize();
    let mut bytes = [0u8; SHA3_256_BYTES];
    bytes.copy_from_slice(&out);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHA3-256("") = a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a
    /// (FIPS 202 known-answer.)
    #[test]
    fn sha3_256_empty_kat() {
        let h = sha3_256(b"");
        assert_eq!(
            hex::encode(h),
            "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a",
        );
    }

    /// SHA3-256("abc") = 3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532
    #[test]
    fn sha3_256_abc_kat() {
        let h = sha3_256(b"abc");
        assert_eq!(
            hex::encode(h),
            "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532",
        );
    }

    #[test]
    fn domain_separation_distinguishes_tag_swap() {
        // H("ab" || "c") with tag separation must differ from H("a" || "bc").
        let a = sha3_256_domain(b"ab", b"c");
        let b = sha3_256_domain(b"a", b"bc");
        assert_ne!(a, b);
    }

    #[test]
    fn determinism() {
        assert_eq!(sha3_256(b"gsx"), sha3_256(b"gsx"));
    }

    /// HKDF-SHA3-256 is deterministic over the same inputs.
    #[test]
    fn hkdf_determinism() {
        let a = hkdf_sha3_256(b"salt", b"ikm", b"info", 32);
        let b = hkdf_sha3_256(b"salt", b"ikm", b"info", 32);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }

    /// Different `info` labels under the same root produce independent outputs.
    /// This is the property Track H relies on to derive nullifier-key,
    /// viewing-seed, and spend-seed independently from one root.
    #[test]
    fn hkdf_info_label_independence() {
        let seed = [0xabu8; 32];
        let nf = hkdf_sha3_256(b"gsx-l2-derive", &seed, b"nullifier-key", 32);
        let view = hkdf_sha3_256(b"gsx-l2-derive", &seed, b"viewing-key", 32);
        let spend = hkdf_sha3_256(b"gsx-l2-derive", &seed, b"spend-key", 32);
        assert_ne!(nf, view);
        assert_ne!(nf, spend);
        assert_ne!(view, spend);
    }

    /// Different `salt` values produce independent outputs even with same ikm.
    #[test]
    fn hkdf_salt_changes_output() {
        let a = hkdf_sha3_256(b"salt-a", b"ikm", b"info", 32);
        let b = hkdf_sha3_256(b"salt-b", b"ikm", b"info", 32);
        assert_ne!(a, b);
    }

    /// Variable-length output: 16, 32, 64, 128 bytes all derive correctly.
    #[test]
    fn hkdf_variable_length() {
        for &len in &[1usize, 16, 32, 64, 128, 256] {
            let out = hkdf_sha3_256(b"salt", b"ikm", b"info", len);
            assert_eq!(out.len(), len);
        }
    }

    /// L2 domain tags are distinct from each other — guards against
    /// copy-paste errors in the Track H confidential-transfer plumbing.
    #[test]
    fn l2_domain_tags_are_distinct() {
        let tags: &[&[u8]] = &[
            SUWAPPU_L2_NOTE_COMMIT_V1,
            SUWAPPU_L2_NULLIFIER_V1,
            SUWAPPU_L2_NF_KEY_V1,
            SUWAPPU_L2_VIEWING_KEY_V1,
            SUWAPPU_L2_ADDRESS_V1,
        ];
        for (i, a) in tags.iter().enumerate() {
            for (j, b) in tags.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "tags {i} and {j} collide");
                }
            }
        }
    }

    /// L2 domain tags produce distinct digests under sha3_256_domain
    /// even when fed identical payload bytes. This is the property the
    /// Track H construction relies on for security separation.
    #[test]
    fn l2_domain_tags_separate_digests() {
        let payload = b"same-payload-different-tag";
        let h_commit = sha3_256_domain(SUWAPPU_L2_NOTE_COMMIT_V1, payload);
        let h_nullifier = sha3_256_domain(SUWAPPU_L2_NULLIFIER_V1, payload);
        let h_nf_key = sha3_256_domain(SUWAPPU_L2_NF_KEY_V1, payload);
        let h_view = sha3_256_domain(SUWAPPU_L2_VIEWING_KEY_V1, payload);
        let h_addr = sha3_256_domain(SUWAPPU_L2_ADDRESS_V1, payload);
        let digests = [h_commit, h_nullifier, h_nf_key, h_view, h_addr];
        for (i, a) in digests.iter().enumerate() {
            for (j, b) in digests.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }
}
