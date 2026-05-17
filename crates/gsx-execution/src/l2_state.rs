//! L2 registry storage shape (Track G G2.2 phase 2).
//!
//! The substrate's bytes-state map at the reserved
//! `l2_registry_address` (per
//! `docs/iq/IQ-006-l2-state-root-commitment-surface.md`) stores a
//! single `L2Registry` value containing:
//!
//! - The chain-pinned VK pair (`aggregation_vk_hash`,
//!   `range_vk_commitment`) per the op-succinct multiBlockVKey
//!   pattern. Rotated via `Intent::SetL2VerifyingKey`.
//! - The per-batch state-roots map (`L2BatchKey ->
//!   L2StateRootRecord`). Each successful
//!   `Intent::CommitL2StateRoot` inserts here.
//!
//! ## Encoding
//!
//! ```text
//! u32::BE(VERSION = 1) ||
//! aggregation_vk_hash (32 B) ||
//! range_vk_commitment (32 B) ||
//! u32::BE(state_root_count) ||
//!   foreach (key, record) in ascending key order:
//!     key.l2_chain_id_hash (32 B) ||
//!     key.batch_id (u64::BE, 8 B) ||
//!     record.state_root (32 B) ||
//!     record.committed_at_l1_height (u64::BE, 8 B) ||
//!     record.vk_hash (32 B) ||
//!     record.da_commitment (32 B)
//! ```
//!
//! Total bytes per state-root entry = 16 (key) + 104 (record) = **120 B**.
//! Header overhead = 4 (version) + 32 (agg_vk) + 32 (range_vk_commit)
//!                 + 4 (count) = **72 B**.
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

/// Current L2 registry encoding version. Bump on incompatible
/// changes; the decoder rejects any other value.
pub const L2_REGISTRY_VERSION: u32 = 1;

/// Length of one encoded `L2StateRootRecord` in bytes.
/// `state_root (32) + committed_at_l1_height (8) + vk_hash (32) +
///  da_commitment (32) = 104`.
pub const L2_STATE_ROOT_RECORD_BYTES: usize = 32 + 8 + 32 + 32;

/// Length of one encoded `(L2BatchKey, L2StateRootRecord)` pair.
/// `key (40) + record (104) = 144`.
///
/// Note: the key is `l2_chain_id_hash (32) + batch_id (8) = 40`.
pub const ENCODED_ENTRY_BYTES: usize = 32 + 8 + L2_STATE_ROOT_RECORD_BYTES;

/// Length of the encoded registry header:
/// `version (4) + agg_vk_hash (32) + range_vk_commit (32) + count (4) = 72`.
pub const ENCODED_HEADER_BYTES: usize = 4 + 32 + 32 + 4;

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

/// The full L2 registry stored at `l2_registry_address`. Owns
/// the chain-pinned VK pair + the per-batch state-roots map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct L2Registry {
    /// SP1 aggregation VK hash. The L1 verifier requires every
    /// `Intent::CommitL2StateRoot::vk_hash` field to equal this
    /// value. Rotated via `Intent::SetL2VerifyingKey`.
    /// `[0u8; 32]` is the "no VK pinned" sentinel.
    pub aggregation_vk_hash: [u8; 32],
    /// Per-batch range-program VK commitment (op-succinct
    /// multiBlockVKey pattern).
    pub range_vk_commitment: [u8; 32],
    /// Per-batch state-roots map keyed by `(l2_chain_id_hash, batch_id)`.
    pub state_roots: BTreeMap<L2BatchKey, L2StateRootRecord>,
}

/// Encode the registry to the canonical on-disk byte sequence.
pub fn encode(reg: &L2Registry) -> Vec<u8> {
    let mut buf =
        Vec::with_capacity(ENCODED_HEADER_BYTES + reg.state_roots.len() * ENCODED_ENTRY_BYTES);
    buf.extend_from_slice(&L2_REGISTRY_VERSION.to_be_bytes());
    buf.extend_from_slice(&reg.aggregation_vk_hash);
    buf.extend_from_slice(&reg.range_vk_commitment);
    buf.extend_from_slice(&(reg.state_roots.len() as u32).to_be_bytes());
    for (key, rec) in &reg.state_roots {
        buf.extend_from_slice(&key.l2_chain_id_hash);
        buf.extend_from_slice(&key.batch_id.to_be_bytes());
        buf.extend_from_slice(&rec.state_root);
        buf.extend_from_slice(&rec.committed_at_l1_height.to_be_bytes());
        buf.extend_from_slice(&rec.vk_hash);
        buf.extend_from_slice(&rec.da_commitment);
    }
    buf
}

/// Decode the byte sequence back into a registry. Empty bytes
/// decode to a default (empty) registry — matches the substrate
/// behavior of "no record at l2_registry_address yet".
pub fn decode(bytes: &[u8]) -> Result<L2Registry, ExecutionError> {
    if bytes.is_empty() {
        return Ok(L2Registry::default());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    if version != L2_REGISTRY_VERSION {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry version mismatch",
        });
    }
    let mut aggregation_vk_hash = [0u8; 32];
    aggregation_vk_hash.copy_from_slice(&bytes[4..36]);
    let mut range_vk_commitment = [0u8; 32];
    range_vk_commitment.copy_from_slice(&bytes[36..68]);
    let count = u32::from_be_bytes(bytes[68..72].try_into().unwrap()) as usize;
    let expected_len = ENCODED_HEADER_BYTES + count * ENCODED_ENTRY_BYTES;
    if bytes.len() != expected_len {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry size mismatch",
        });
    }
    let mut state_roots = BTreeMap::new();
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
        if state_roots.insert(key, rec).is_some() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::l2_registry_address(),
                reason: "L2 registry has duplicate keys",
            });
        }
    }
    Ok(L2Registry {
        aggregation_vk_hash,
        range_vk_commitment,
        state_roots,
    })
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

    fn empty_registry() -> L2Registry {
        L2Registry::default()
    }

    #[test]
    fn empty_registry_round_trips() {
        let r = empty_registry();
        let bytes = encode(&r);
        // 72 B header (version + agg_vk + range_vk + count), no entries.
        assert_eq!(bytes.len(), ENCODED_HEADER_BYTES);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn empty_bytes_decodes_to_empty_registry() {
        let r = decode(&[]).unwrap();
        assert!(r.state_roots.is_empty());
        assert_eq!(r.aggregation_vk_hash, [0u8; 32]);
        assert_eq!(r.range_vk_commitment, [0u8; 32]);
    }

    #[test]
    fn vk_pair_round_trips() {
        let r = L2Registry {
            aggregation_vk_hash: [0xab; 32],
            range_vk_commitment: [0xcd; 32],
            state_roots: BTreeMap::new(),
        };
        let bytes = encode(&r);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
        assert_eq!(r2.aggregation_vk_hash, [0xab; 32]);
        assert_eq!(r2.range_vk_commitment, [0xcd; 32]);
    }

    #[test]
    fn single_entry_round_trips() {
        let mut r = empty_registry();
        r.state_roots.insert(key(0x01, 5), record(0xab));
        let bytes = encode(&r);
        assert_eq!(bytes.len(), ENCODED_HEADER_BYTES + ENCODED_ENTRY_BYTES);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn vk_pair_and_entries_round_trip_together() {
        let mut r = L2Registry {
            aggregation_vk_hash: [0xa0; 32],
            range_vk_commitment: [0xa1; 32],
            state_roots: BTreeMap::new(),
        };
        r.state_roots.insert(key(0x01, 0), record(0xa1));
        r.state_roots.insert(key(0x02, 7), record(0xb2));
        let bytes = encode(&r);
        assert_eq!(bytes.len(), ENCODED_HEADER_BYTES + 2 * ENCODED_ENTRY_BYTES);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn encoding_is_deterministic_across_runs() {
        let mut r = empty_registry();
        r.state_roots.insert(key(0x01, 1), record(0xab));
        r.state_roots.insert(key(0x02, 2), record(0xcd));
        let bytes_a = encode(&r);
        let bytes_b = encode(&r);
        assert_eq!(bytes_a, bytes_b);
    }

    #[test]
    fn encoding_is_ascending_key_order() {
        let mut a = empty_registry();
        a.state_roots.insert(key(0x02, 0), record(0xb1));
        a.state_roots.insert(key(0x01, 0), record(0xa1));
        let mut b = empty_registry();
        b.state_roots.insert(key(0x01, 0), record(0xa1));
        b.state_roots.insert(key(0x02, 0), record(0xb1));
        assert_eq!(encode(&a), encode(&b));
    }

    #[test]
    fn decode_rejects_size_mismatch() {
        // Claim 2 entries but provide bytes for 1.
        let mut bytes = vec![0, 0, 0, L2_REGISTRY_VERSION as u8]; // version
        bytes.extend_from_slice(&[0u8; 32]); // agg_vk
        bytes.extend_from_slice(&[0u8; 32]); // range_vk
        bytes.extend_from_slice(&[0, 0, 0, 2]); // count = 2
        bytes.extend_from_slice(&[0u8; ENCODED_ENTRY_BYTES]); // only 1 entry payload
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_short_header() {
        // 50 bytes — less than the 72-byte header.
        assert!(matches!(
            decode(&[0u8; 50]),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_wrong_version() {
        // Header with version=2 (current is 1).
        let mut bytes = vec![0, 0, 0, 2]; // version = 2
        bytes.extend_from_slice(&[0u8; 32]);
        bytes.extend_from_slice(&[0u8; 32]);
        bytes.extend_from_slice(&[0, 0, 0, 0]); // count = 0
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn entry_size_constants_match_packed_layout() {
        assert_eq!(L2_STATE_ROOT_RECORD_BYTES, 32 + 8 + 32 + 32);
        assert_eq!(ENCODED_ENTRY_BYTES, 32 + 8 + L2_STATE_ROOT_RECORD_BYTES);
        assert_eq!(ENCODED_HEADER_BYTES, 4 + 32 + 32 + 4);
    }
}
