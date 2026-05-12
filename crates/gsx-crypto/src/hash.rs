//! Hashing primitives.
//!
//! SHA3-256 (FIPS 202) is the canonical hash on the LTP commitment surface
//! (paper §10.2) and the SCION-routed transport layer. Poseidon2 is provided
//! for arithmetic-friendly contexts (state-tree leaf encodings, future PlonK
//! circuits) but is intentionally **not** used on the LTP integrity surface.

use sha3::{Digest, Sha3_256};

/// Length of a SHA3-256 digest in bytes.
pub const SHA3_256_BYTES: usize = 32;

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
}
