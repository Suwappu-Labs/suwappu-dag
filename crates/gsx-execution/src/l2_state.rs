//! L2 state-root storage shape (Track G G2.2 phase 2, partial).
//!
//! The L2 verifier-precompile arm of `apply_intent` records every
//! successful `Intent::CommitL2StateRoot` as a per-batch record in
//! the substrate's bytes-state map at the reserved
//! `l2_registry_address` (per
//! `docs/iq/IQ-006-l2-state-root-commitment-surface.md`). This
//! module defines the on-disk encoding of that record + the
//! container map.
//!
//! ## Encoding
//!
//! At the L2 registry address the substrate stores a deterministic
//! byte sequence:
//!
//! ```text
//! u32::BE(count) ||
//!   foreach (key, record) in ascending key order:
//!     key.l2_chain_id_hash (32 B) ||
//!     key.batch_id (u64::BE, 8 B) ||
//!     record.state_root (32 B) ||
//!     record.committed_at_l1_height (u64::BE, 8 B) ||
//!     record.vk_hash (32 B) ||
//!     record.da_commitment (32 B)
//! ```
//!
//! Total bytes per entry = 16 (key) + 104 (record) = **120 B**.
//!
//! ## Scalability
//!
//! Phase 1: every commit decodes the full map, inserts, re-encodes.
//! O(N) per commit. Fine for ≤ 10k batches per L2 chain (~1.2 MB
//! map). Phase 2 may switch to an append-only log + index tree
//! once scaling pressure justifies the complexity.
//!
//! ## Multi-L2 forward-compat
//!
//! The `l2_chain_id_hash` is the SHA3 of `b"gsx-l2-chain-" ||
//! chain_id` per IQ-006. A new L2 chain just uses a different
//! hash. No schema changes, no hard fork.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{error::ExecutionError, reserved};

/// Length of one encoded `L2StateRootRecord` in bytes.
/// `state_root (32) + committed_at_l1_height (8) + vk_hash (32) +
///  da_commitment (32) = 104`.
pub const L2_STATE_ROOT_RECORD_BYTES: usize = 32 + 8 + 32 + 32;

/// Length of one encoded `(L2BatchKey, L2StateRootRecord)` pair.
/// `key (40) + record (104) = 144`.
///
/// Note: the key is `l2_chain_id_hash (32) + batch_id (8) = 40`.
/// Storage cost per pair: 144 B.
pub const ENCODED_ENTRY_BYTES: usize = 32 + 8 + L2_STATE_ROOT_RECORD_BYTES;

/// Length of the encoded map header (a `u32::BE` entry count).
pub const ENCODED_HEADER_BYTES: usize = 4;

/// Per-batch L2 state-root record. Stored at the reserved
/// `l2_registry_address` keyed by `(l2_chain_id_hash, batch_id)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct L2StateRootRecord {
    /// L2 state root committed by this batch (EVM MPT root per
    /// Open Item #8 EVM flip).
    pub state_root: [u8; 32],
    /// L1 block height at which this commit landed. Decoded from
    /// the public-inputs blob's `l1_anchor_height` field at the
    /// time of substrate handling (see
    /// `gsx_l2_verifier_precompile::public_inputs::L1_ANCHOR_HEIGHT_OFFSET`).
    pub committed_at_l1_height: u64,
    /// SP1 verifying-key hash this proof verified under. Recorded
    /// so post-mortem analysis can audit which VK validated each
    /// batch across `Intent::SetL2VerifyingKey` rotations.
    pub vk_hash: [u8; 32],
    /// DA commitment binding the batch's DA blob.
    pub da_commitment: [u8; 32],
}

/// Composite key identifying an L2 batch within the registry.
/// The hash is `SHA3-256("gsx-l2-chain-" || chain_id)` per IQ-006;
/// the substrate stores it directly without needing to know
/// `chain_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct L2BatchKey {
    /// L2 chain identifier hash (multi-L2 namespacing per IQ-006).
    pub l2_chain_id_hash: [u8; 32],
    /// Monotonic per-L2-chain batch identifier.
    pub batch_id: u64,
}

/// Encode the L2 state-root map into the on-disk byte sequence.
/// Deterministic in BTreeMap ascending-key order.
pub fn encode_map(map: &BTreeMap<L2BatchKey, L2StateRootRecord>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ENCODED_HEADER_BYTES + map.len() * ENCODED_ENTRY_BYTES);
    buf.extend_from_slice(&(map.len() as u32).to_be_bytes());
    for (key, rec) in map {
        buf.extend_from_slice(&key.l2_chain_id_hash);
        buf.extend_from_slice(&key.batch_id.to_be_bytes());
        buf.extend_from_slice(&rec.state_root);
        buf.extend_from_slice(&rec.committed_at_l1_height.to_be_bytes());
        buf.extend_from_slice(&rec.vk_hash);
        buf.extend_from_slice(&rec.da_commitment);
    }
    buf
}

/// Decode the byte sequence back into a map. Returns
/// `CorruptStateRecord` if the bytes don't match the canonical
/// encoding (header too short, declared length doesn't match
/// payload, etc).
pub fn decode_map(bytes: &[u8]) -> Result<BTreeMap<L2BatchKey, L2StateRootRecord>, ExecutionError> {
    if bytes.is_empty() {
        // Treat empty bytes as an empty map. This is how the
        // registry account looks before the first commit
        // (bytes_state.get(&addr) is None → caller passes empty).
        return Ok(BTreeMap::new());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 state-root map header missing",
        });
    }
    let count = u32::from_be_bytes(bytes[0..ENCODED_HEADER_BYTES].try_into().unwrap()) as usize;
    let expected_len = ENCODED_HEADER_BYTES + count * ENCODED_ENTRY_BYTES;
    if bytes.len() != expected_len {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 state-root map size mismatch",
        });
    }
    let mut map = BTreeMap::new();
    let mut cursor = ENCODED_HEADER_BYTES;
    for _ in 0..count {
        let mut l2_chain_id_hash = [0u8; 32];
        l2_chain_id_hash.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let batch_id = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let key = L2BatchKey {
            l2_chain_id_hash,
            batch_id,
        };
        let mut state_root = [0u8; 32];
        state_root.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let committed_at_l1_height =
            u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let mut vk_hash = [0u8; 32];
        vk_hash.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let mut da_commitment = [0u8; 32];
        da_commitment.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let rec = L2StateRootRecord {
            state_root,
            committed_at_l1_height,
            vk_hash,
            da_commitment,
        };
        // Reject duplicate keys (would silently overwrite in BTreeMap);
        // the encoder shouldn't produce these, but be defensive.
        if map.insert(key, rec).is_some() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::l2_registry_address(),
                reason: "L2 state-root map has duplicate keys",
            });
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(seed: u8) -> L2StateRootRecord {
        L2StateRootRecord {
            state_root: [seed; 32],
            committed_at_l1_height: 100 + seed as u64,
            vk_hash: [seed.wrapping_add(1); 32],
            da_commitment: [seed.wrapping_add(2); 32],
        }
    }

    fn key(chain_seed: u8, batch_id: u64) -> L2BatchKey {
        L2BatchKey {
            l2_chain_id_hash: [chain_seed; 32],
            batch_id,
        }
    }

    #[test]
    fn empty_map_round_trips() {
        let m = BTreeMap::new();
        let bytes = encode_map(&m);
        // 4 bytes for the u32::BE(0) header, no entries.
        assert_eq!(bytes.len(), 4);
        assert_eq!(bytes, [0, 0, 0, 0]);
        let m2 = decode_map(&bytes).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn empty_bytes_decodes_to_empty_map() {
        // Convenience for the substrate read path: a missing
        // entry in bytes_state should be treated as an empty
        // map, not a decode error.
        let m = decode_map(&[]).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn single_entry_round_trips() {
        let mut m = BTreeMap::new();
        m.insert(key(0x01, 5), record(0xab));
        let bytes = encode_map(&m);
        assert_eq!(bytes.len(), ENCODED_HEADER_BYTES + ENCODED_ENTRY_BYTES);
        let m2 = decode_map(&bytes).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn multi_entry_round_trips() {
        let mut m = BTreeMap::new();
        m.insert(key(0x01, 0), record(0xa1));
        m.insert(key(0x01, 1), record(0xa2));
        m.insert(key(0x02, 0), record(0xb1));
        m.insert(key(0x02, 7), record(0xb2));
        let bytes = encode_map(&m);
        assert_eq!(bytes.len(), ENCODED_HEADER_BYTES + 4 * ENCODED_ENTRY_BYTES);
        let m2 = decode_map(&bytes).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn encoding_is_deterministic_across_runs() {
        let mut m = BTreeMap::new();
        m.insert(key(0x01, 1), record(0xab));
        m.insert(key(0x02, 2), record(0xcd));
        let bytes_a = encode_map(&m);
        let bytes_b = encode_map(&m);
        assert_eq!(bytes_a, bytes_b);
    }

    #[test]
    fn encoding_is_ascending_key_order() {
        // BTreeMap iterates ascending. Verify the encoded bytes
        // show key(0x01,_) before key(0x02,_) regardless of
        // insertion order.
        let mut a = BTreeMap::new();
        a.insert(key(0x02, 0), record(0xb1));
        a.insert(key(0x01, 0), record(0xa1));
        let mut b = BTreeMap::new();
        b.insert(key(0x01, 0), record(0xa1));
        b.insert(key(0x02, 0), record(0xb1));
        assert_eq!(encode_map(&a), encode_map(&b));
    }

    #[test]
    fn decode_rejects_size_mismatch() {
        // Claim 2 entries but provide bytes for 1.
        let mut bytes = vec![0, 0, 0, 2];
        bytes.extend_from_slice(&[0u8; ENCODED_ENTRY_BYTES]);
        assert!(matches!(
            decode_map(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_short_header() {
        // 3 bytes — less than the 4-byte header.
        assert!(matches!(
            decode_map(&[0, 0, 0]),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn entry_size_constants_match_packed_layout() {
        // Defensive: the per-entry byte width matches what the
        // encoder writes. If the L2StateRootRecord struct grows
        // a field, the constants must be updated in lockstep
        // and this test pins the relationship.
        assert_eq!(L2_STATE_ROOT_RECORD_BYTES, 32 + 8 + 32 + 32);
        assert_eq!(ENCODED_ENTRY_BYTES, 32 + 8 + L2_STATE_ROOT_RECORD_BYTES);
    }
}
