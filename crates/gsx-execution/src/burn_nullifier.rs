//! L2 burn-nullifier registry storage (Track G G3.2
//! hardening: double-spend defense).
//!
//! Every successful `Intent::L2BurnProven` writes a unique
//! `burn_id` into this set. Subsequent burns with the same
//! `burn_id` reject with `ExecutionError::L2BurnAlreadyClaimed`.
//!
//! ## What it defends against
//!
//! Before this nullifier set, a caller with a valid
//! `L2BurnProven` Intent could submit it repeatedly: each
//! repetition would drain the bridge escrow (up to its
//! balance). The merkle_path is still a byte-shape stub
//! (full Merkle inclusion proof verification lands in G2.2
//! phase 3), so without the nullifier set there's no
//! deterministic dedup of burn claims.
//!
//! ## burn_id derivation
//!
//! ```text
//! burn_id = BLAKE3("gsx-burn-v1" ||
//!                  l2_chain_id_hash (32) ||
//!                  u64_be(batch_id) ||
//!                  recipient (20) ||
//!                  u128_be(amount) ||
//!                  u32_be(merkle_path.len()) ||
//!                  merkle_path ||
//!                  asset_id_present (1) ||
//!                  asset_id (32, if present))
//! ```
//!
//! Two L2BurnProven Intents with the same `burn_id` ARE
//! the same logical burn (same chain, batch, recipient,
//! amount, witness, asset). Replaying is detected
//! deterministically.

use std::collections::BTreeSet;

use blake3::Hasher;

use crate::{
    error::ExecutionError,
    reserved,
    substrate::{Address, Balance},
};

/// Current burn-nullifier encoding version.
pub const BURN_NULLIFIER_VERSION: u32 = 1;

/// Encoded-header bytes: `version (4) + count (4) = 8`.
pub const ENCODED_HEADER_BYTES: usize = 4 + 4;

/// Encoded bytes per entry: just the 32-byte `burn_id`.
pub const ENTRY_BYTES: usize = 32;

/// Compute the canonical `burn_id` for an L2BurnProven
/// Intent. Domain-tagged BLAKE3 over every field that
/// distinguishes one burn from another.
pub fn burn_id(
    l2_chain_id_hash: &[u8; 32],
    batch_id: u64,
    recipient: &Address,
    amount: Balance,
    merkle_path: &[u8],
    asset_id: &Option<[u8; 32]>,
) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(b"gsx-burn-v1");
    h.update(l2_chain_id_hash);
    h.update(&batch_id.to_be_bytes());
    h.update(recipient);
    h.update(&amount.to_be_bytes());
    h.update(&(merkle_path.len() as u32).to_be_bytes());
    h.update(merkle_path);
    match asset_id {
        Some(id) => {
            h.update(&[1u8]);
            h.update(id);
        }
        None => {
            h.update(&[0u8]);
        }
    }
    *h.finalize().as_bytes()
}

/// Encode the burn-nullifier set to the canonical byte
/// sequence. BTreeSet iteration is deterministic.
pub fn encode(set: &BTreeSet<[u8; 32]>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ENCODED_HEADER_BYTES + set.len() * ENTRY_BYTES);
    buf.extend_from_slice(&BURN_NULLIFIER_VERSION.to_be_bytes());
    buf.extend_from_slice(&(set.len() as u32).to_be_bytes());
    for id in set {
        buf.extend_from_slice(id);
    }
    buf
}

/// Decode the byte sequence back into the burn-nullifier set.
pub fn decode(bytes: &[u8]) -> Result<BTreeSet<[u8; 32]>, ExecutionError> {
    if bytes.is_empty() {
        return Ok(BTreeSet::new());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::burn_nullifier_registry_address(),
            reason: "burn nullifier header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    if version != BURN_NULLIFIER_VERSION {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::burn_nullifier_registry_address(),
            reason: "burn nullifier version mismatch",
        });
    }
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let expected_len = ENCODED_HEADER_BYTES + count * ENTRY_BYTES;
    if bytes.len() != expected_len {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::burn_nullifier_registry_address(),
            reason: "burn nullifier size mismatch",
        });
    }
    let mut set = BTreeSet::new();
    let mut cursor = ENCODED_HEADER_BYTES;
    for _ in 0..count {
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        set.insert(id);
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address {
        [b; 20]
    }

    #[test]
    fn burn_id_deterministic() {
        let id1 = burn_id(&[0u8; 32], 1, &addr(1), 100, &[0xab; 32], &None);
        let id2 = burn_id(&[0u8; 32], 1, &addr(1), 100, &[0xab; 32], &None);
        assert_eq!(id1, id2);
    }

    #[test]
    fn burn_id_distinct_for_different_fields() {
        let base = burn_id(&[0u8; 32], 1, &addr(1), 100, &[0xab; 32], &None);
        // Different batch.
        assert_ne!(
            base,
            burn_id(&[0u8; 32], 2, &addr(1), 100, &[0xab; 32], &None)
        );
        // Different recipient.
        assert_ne!(
            base,
            burn_id(&[0u8; 32], 1, &addr(2), 100, &[0xab; 32], &None)
        );
        // Different amount.
        assert_ne!(
            base,
            burn_id(&[0u8; 32], 1, &addr(1), 200, &[0xab; 32], &None)
        );
        // Different merkle_path.
        assert_ne!(
            base,
            burn_id(&[0u8; 32], 1, &addr(1), 100, &[0xcd; 32], &None)
        );
        // Different chain.
        assert_ne!(
            base,
            burn_id(&[0xaa; 32], 1, &addr(1), 100, &[0xab; 32], &None)
        );
        // Asset Some vs None.
        assert_ne!(
            base,
            burn_id(&[0u8; 32], 1, &addr(1), 100, &[0xab; 32], &Some([0xde; 32]))
        );
        // Different asset_id.
        assert_ne!(
            burn_id(&[0u8; 32], 1, &addr(1), 100, &[0xab; 32], &Some([0xde; 32])),
            burn_id(&[0u8; 32], 1, &addr(1), 100, &[0xab; 32], &Some([0xfe; 32])),
        );
    }

    /// Length-prefix on merkle_path defends against
    /// `[0x01]||0x02` vs `[0x01, 0x02]` style collisions.
    #[test]
    fn burn_id_length_prefix_defense() {
        let a = burn_id(&[0u8; 32], 1, &addr(1), 100, &[0x01], &None);
        let b = burn_id(&[0u8; 32], 1, &addr(1), 100, &[0x01, 0x02], &None);
        assert_ne!(a, b);
    }

    #[test]
    fn empty_set_round_trip() {
        let s: BTreeSet<[u8; 32]> = BTreeSet::new();
        let bytes = encode(&s);
        assert_eq!(decode(&bytes).unwrap(), s);
    }

    #[test]
    fn single_entry_round_trip() {
        let mut s = BTreeSet::new();
        s.insert([0xaa; 32]);
        let bytes = encode(&s);
        assert_eq!(decode(&bytes).unwrap(), s);
    }

    #[test]
    fn multi_entry_round_trip_deterministic_order() {
        let mut s1 = BTreeSet::new();
        s1.insert([0x01; 32]);
        s1.insert([0x02; 32]);
        s1.insert([0x03; 32]);

        // Insertion order in a BTreeSet doesn't matter — the
        // encoded bytes are determined by iteration order
        // (ascending).
        let mut s2 = BTreeSet::new();
        s2.insert([0x03; 32]);
        s2.insert([0x01; 32]);
        s2.insert([0x02; 32]);

        assert_eq!(encode(&s1), encode(&s2));
        assert_eq!(decode(&encode(&s1)).unwrap(), s1);
    }

    #[test]
    fn decode_rejects_short_header() {
        let bytes = vec![0u8; 4];
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_version_mismatch() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&999u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_size_mismatch() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&BURN_NULLIFIER_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        // Header claims 1 entry but no bytes follow.
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }
}
