//! Bridge header attestation preimage + digest + ML-DSA signing.
//!
//! This module builds the cross-language ground-truth preimage that the
//! Suwappu bridge forge contracts emit and that validators sign as a
//! **side-attestation** over a gsx-dag block header. It is the Rust half of
//! a cross-language packing contract: the bytes produced here must reproduce,
//! byte-for-byte, the `abi.encodePacked` preimage emitted on the Solidity
//! (forge) side.
//!
//! ## Trust model (honest, non-negotiable)
//!
//! This is a **validator-quorum side-attestation** (sync-committee trust
//! class: safe under an honest >2/3-stake assumption). It is **NOT** a
//! consensus light client, **NOT** trustless, and **NOT** end-to-end
//! post-quantum. The functions here are pure: nothing in this module is
//! wired into the consensus loop, the daemon, or the mint path. The
//! oracle/registry on the contract side is currently UNFED — validators do
//! not yet sign these headers. Replacing a single oracle key with a k-of-N
//! ML-DSA operator set still leaves a *trusted* set; it merely removes the
//! single-key dependency. Wiring into consensus and the mint path is a
//! later slice.
//!
//! ## Packing
//!
//! The preimage is `abi.encodePacked`-equivalent and is exactly 148 bytes:
//!
//! ```text
//! HEADER_DOMAIN (32) || network_id (32) || oracle (20)
//!   || block_number-as-uint256-big-endian (32) || state_root (32)
//! ```
//!
//! `block_number` is a `u64` widened to a 32-byte big-endian `uint256`
//! (24 leading zero bytes), matching Solidity's `abi.encodePacked(uint256)`.

use gsx_crypto::mldsa::{sign, verify, PublicKey, SecretKey, Signature};

/// Total length, in bytes, of the bridge-header attestation preimage.
pub const HEADER_PREIMAGE_LEN: usize = 148;

/// Domain-separation tag for the bridge-header attestation surface.
///
/// This is `keccak256("SUWAPPU_GSXDAG_HEADER_V1")` (the ASCII string, no
/// trailing NUL), hard-pinned here as the 32-byte literal so this crate
/// carries no runtime keccak dependency. The value is verified to equal the
/// Solidity constant `HEADER_DOMAIN` by the `domain_matches_keccak256`
/// test below, which recomputes `Keccak256` of the same ASCII string and
/// asserts equality. The first 32 bytes of the cross-language golden
/// preimage (`forge_golden_preimage_matches`) are also exactly this value.
pub const HEADER_DOMAIN: [u8; 32] = [
    0xc7, 0x0c, 0x21, 0xeb, 0xc7, 0x9f, 0x8a, 0x20, 0x43, 0x34, 0x57, 0xa7, 0x0c, 0xf2, 0x98, 0x5f,
    0x05, 0xe7, 0x0b, 0x01, 0x7c, 0xbd, 0x95, 0xf3, 0x28, 0xe3, 0xb2, 0xa8, 0x72, 0x1e, 0xbd, 0x3a,
];

/// Build the bridge-header attestation preimage.
///
/// Produces exactly [`HEADER_PREIMAGE_LEN`] (148) bytes:
/// `HEADER_DOMAIN(32) || network_id(32) || oracle(20)
///   || block_number-as-uint256-BE(32) || state_root(32)`.
///
/// `network_id` is a big-endian `uint256`. `block_number` is widened from a
/// `u64` to a 32-byte big-endian `uint256` (24 leading zero bytes), matching
/// `abi.encodePacked(uint256(blockNumber))` on the Solidity side.
pub fn header_preimage(
    network_id: [u8; 32],
    oracle: [u8; 20],
    block_number: u64,
    state_root: [u8; 32],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_PREIMAGE_LEN);
    out.extend_from_slice(&HEADER_DOMAIN);
    out.extend_from_slice(&network_id);
    out.extend_from_slice(&oracle);
    // u64 -> uint256 big-endian: 24 zero bytes followed by the 8 BE bytes.
    let mut block_number_u256 = [0u8; 32];
    block_number_u256[24..].copy_from_slice(&block_number.to_be_bytes());
    out.extend_from_slice(&block_number_u256);
    out.extend_from_slice(&state_root);
    debug_assert_eq!(out.len(), HEADER_PREIMAGE_LEN);
    out
}

/// Compute the BLAKE3 digest of the bridge-header attestation preimage.
///
/// The digest is `blake3::hash(&header_preimage(..))`. This is the message a
/// validator's ML-DSA secret key signs.
pub fn header_digest(
    network_id: [u8; 32],
    oracle: [u8; 20],
    block_number: u64,
    state_root: [u8; 32],
) -> [u8; 32] {
    let preimage = header_preimage(network_id, oracle, block_number, state_root);
    *blake3::hash(&preimage).as_bytes()
}

/// Produce an ML-DSA-65 detached signature over a bridge-header digest.
///
/// Returns the raw detached-signature bytes. Signing over a freshly produced
/// digest with a valid secret key is infallible, so the inner `Result` is
/// unwrapped.
pub fn sign_header(digest: &[u8; 32], sk: &SecretKey) -> Vec<u8> {
    sign(digest, sk)
        .expect("ml-dsa-65 detached_sign over a valid secret key is infallible")
        .as_bytes()
        .to_vec()
}

/// A single validator's ML-DSA side-attestation over a gsx-dag block header.
///
/// This is the unit a relayer collects from each validator's RPC and, once it
/// holds a set whose stake exceeds the on-chain >2/3 threshold, submits to the
/// destination `GsxDagQuorumHeaderOracle.submitHeader`. It carries the signing
/// validator's ML-DSA-65 public key so the relayer can sort by
/// `keccak256(pubkey)` (the contract's strictly-increasing dedup order) and so
/// the on-chain registry can match it to a registered, staked member.
///
/// Honest trust model: an attestation is a validator's *claim* about the block
/// header it locally finalized; it is **not** a proof. `state_root` is the
/// gsx-dag BLAKE3 L1 state root (`ExecutionReport::post_root`), which is **not**
/// an EVM-MPT root and is therefore **not** storage-provable today — the header
/// is an opaque finalized-round anchor. Safety rests on an honest >2/3-stake
/// quorum, not on cryptographic source-state inclusion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderAttestation {
    /// The committed DAG round whose execution produced `state_root`.
    pub block_number: u64,
    /// The gsx-dag BLAKE3 L1 state root after executing the block at
    /// `block_number` (`ExecutionReport::post_root`).
    pub state_root: [u8; 32],
    /// The attesting Authority Ring member id (matches the on-chain registry).
    pub authority_id: u32,
    /// The attesting validator's ML-DSA-65 public-key bytes.
    pub pubkey: Vec<u8>,
    /// Detached ML-DSA-65 signature over `header_digest(network_id, oracle,
    /// block_number, state_root)`.
    pub signature: Vec<u8>,
}

impl HeaderAttestation {
    /// Build and sign an attestation for `(block_number, state_root)` bound to
    /// `(network_id, oracle)` under the validator's ML-DSA keypair.
    pub fn create(
        network_id: [u8; 32],
        oracle: [u8; 20],
        block_number: u64,
        state_root: [u8; 32],
        authority_id: u32,
        pubkey: &PublicKey,
        sk: &SecretKey,
    ) -> Self {
        let digest = header_digest(network_id, oracle, block_number, state_root);
        Self {
            block_number,
            state_root,
            authority_id,
            pubkey: pubkey.as_bytes().to_vec(),
            signature: sign_header(&digest, sk),
        }
    }

    /// Verify this attestation's signature binds its `(block_number,
    /// state_root)` to `(network_id, oracle)` under its own carried public key.
    ///
    /// Returns `false` (never panics) on malformed key/signature bytes. This
    /// checks only the signature; it does NOT check that `pubkey` belongs to a
    /// registered, staked validator — the on-chain registry / relayer does that.
    pub fn verify(&self, network_id: [u8; 32], oracle: [u8; 20]) -> bool {
        let digest = header_digest(network_id, oracle, self.block_number, self.state_root);
        match (
            PublicKey::from_bytes(&self.pubkey),
            Signature::from_bytes(&self.signature),
        ) {
            (Ok(pk), Ok(sig)) => verify(&digest, &sig, &pk).is_ok(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsx_crypto::mldsa::{keypair, verify, Signature};
    use sha3::{Digest, Keccak256};

    /// The cross-language golden preimage emitted by the forge side, as the
    /// literal `abi.encodePacked` hex string (148 bytes). This is the
    /// load-bearing ground truth: the Rust packing must reproduce these exact
    /// bytes, so this string is embedded verbatim and decoded, NOT
    /// regenerated from the scalars.
    const FORGE_GOLDEN_PREIMAGE_HEX: &str = "c70c21ebc79f8a20433457a70cf2985f05e70b017cbd95f328e3b2a8721ebd3aff431b3851ff00be6b5a4bd9b67e7d4118300693937865dfe75847dfd7cdd78a00000000000000000000000000000000000000a10000000000000000000000000000000000000000000000000000000000001092b33d996c809e9a90d13c6a64fec28295f01c27d356843d9defc491bfda42f692";

    /// Golden-vector scalars (the same values the forge side packed).
    fn golden_scalars() -> ([u8; 32], [u8; 20], u64, [u8; 32]) {
        let network_id = hex32("ff431b3851ff00be6b5a4bd9b67e7d4118300693937865dfe75847dfd7cdd78a");
        let mut oracle = [0u8; 20];
        oracle[19] = 0xa1; // 0x00..00a1
        let block_number: u64 = 4242;
        let state_root = hex32("b33d996c809e9a90d13c6a64fec28295f01c27d356843d9defc491bfda42f692");
        (network_id, oracle, block_number, state_root)
    }

    fn hex32(s: &str) -> [u8; 32] {
        let bytes = decode_hex(s);
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        out
    }

    fn decode_hex(s: &str) -> Vec<u8> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        assert!(s.len() % 2 == 0, "odd-length hex");
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex digit"))
            .collect()
    }

    /// Load-bearing cross-language packing proof: the Rust preimage must equal
    /// the forge-emitted bytes byte-for-byte.
    #[test]
    fn forge_golden_preimage_matches() {
        let (network_id, oracle, block_number, state_root) = golden_scalars();
        let preimage = header_preimage(network_id, oracle, block_number, state_root);
        let expected = decode_hex(FORGE_GOLDEN_PREIMAGE_HEX);
        assert_eq!(
            expected.len(),
            HEADER_PREIMAGE_LEN,
            "golden vector is 148 bytes"
        );
        assert_eq!(
            preimage, expected,
            "Rust abi.encodePacked preimage must match the forge bytes byte-for-byte"
        );
    }

    /// The hard-pinned `HEADER_DOMAIN` literal must equal
    /// `keccak256("SUWAPPU_GSXDAG_HEADER_V1")` recomputed at test time.
    #[test]
    fn domain_matches_keccak256() {
        let mut hasher = Keccak256::new();
        hasher.update(b"SUWAPPU_GSXDAG_HEADER_V1");
        let computed: [u8; 32] = hasher.finalize().into();
        assert_eq!(
            computed, HEADER_DOMAIN,
            "HEADER_DOMAIN must equal keccak256 of the ASCII domain string"
        );
        // And it is exactly the first 32 bytes of the forge golden preimage.
        let golden = decode_hex(FORGE_GOLDEN_PREIMAGE_HEX);
        assert_eq!(&golden[..32], &HEADER_DOMAIN);
    }

    /// `header_digest` must equal BLAKE3 of the golden preimage.
    #[test]
    fn digest_is_blake3_of_preimage() {
        let (network_id, oracle, block_number, state_root) = golden_scalars();
        let digest = header_digest(network_id, oracle, block_number, state_root);
        let expected = *blake3::hash(&decode_hex(FORGE_GOLDEN_PREIMAGE_HEX)).as_bytes();
        assert_eq!(digest, expected, "digest must be blake3(preimage)");
    }

    /// Fresh ML-DSA-65 keypair: sign the header digest, then verify the
    /// detached signature roundtrips against the public key.
    #[test]
    fn sign_verify_roundtrip() {
        let (pk, sk) = keypair();
        let (network_id, oracle, block_number, state_root) = golden_scalars();
        let digest = header_digest(network_id, oracle, block_number, state_root);

        let sig_bytes = sign_header(&digest, &sk);
        let sig = Signature::from_bytes(&sig_bytes).expect("well-formed detached signature");
        verify(&digest, &sig, &pk).expect("fresh-keypair attestation must verify");

        // A tampered digest must not verify under the same signature.
        let mut tampered = digest;
        tampered[0] ^= 0x01;
        assert!(
            verify(&tampered, &sig, &pk).is_err(),
            "signature must not verify over a different digest"
        );
    }

    /// A `HeaderAttestation` binds to the golden digest (non-vacuous: asserted
    /// against the embedded golden preimage's BLAKE3, not a re-derivation),
    /// verifies under the correct `(network_id, oracle)`, and fails under a
    /// wrong oracle or a tampered state root.
    #[test]
    fn attestation_binds_to_golden_digest_and_verifies() {
        let (pk, sk) = keypair();
        let (network_id, oracle, block_number, state_root) = golden_scalars();
        let att =
            HeaderAttestation::create(network_id, oracle, block_number, state_root, 7, &pk, &sk);

        // Non-vacuous: the digest this attestation signed equals BLAKE3 of the
        // embedded forge golden preimage.
        let golden_digest = *blake3::hash(&decode_hex(FORGE_GOLDEN_PREIMAGE_HEX)).as_bytes();
        let signed_digest = header_digest(network_id, oracle, att.block_number, att.state_root);
        assert_eq!(
            signed_digest, golden_digest,
            "attestation must sign the golden digest"
        );

        assert_eq!(att.authority_id, 7);
        assert!(
            att.verify(network_id, oracle),
            "honest attestation must verify"
        );

        // Bound to a specific oracle: a different oracle address must not verify.
        let mut other_oracle = oracle;
        other_oracle[19] ^= 0x01;
        assert!(
            !att.verify(network_id, other_oracle),
            "attestation must not verify against a different oracle binding"
        );

        // Tampering the state root must break verification.
        let mut forged = att.clone();
        forged.state_root[0] ^= 0x01;
        assert!(
            !forged.verify(network_id, oracle),
            "tampered state root must not verify"
        );

        // Malformed signature/pubkey bytes return false, never panic.
        let mut malformed = att.clone();
        malformed.signature.truncate(4);
        assert!(!malformed.verify(network_id, oracle));
    }
}
